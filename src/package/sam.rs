use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use crate::error::{SpmError, SpmResult};
use crate::types::Manifest;
use crate::util::hash;

const SAM_MAGIC: &[u8; 4] = b"SAM1";

pub fn read_manifest(path: &str) -> SpmResult<Manifest> {
    let mut file = fs::File::open(path).map_err(|e| SpmError::invalid_format(format!("Failed to open .sam file {path}: {e}")))?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).map_err(|e| SpmError::invalid_format(format!("Failed to read magic: {e}")))?;
    if &magic != SAM_MAGIC {
        return Err(SpmError::invalid_format("Invalid .sam file: bad magic"));
    }

    let mut len_buf = [0u8; 4];
    file.read_exact(&mut len_buf).map_err(|e| SpmError::invalid_format(format!("Failed to read manifest length: {e}")))?;
    let manifest_len = u32::from_le_bytes(len_buf) as usize;

    let mut manifest_bytes = vec![0u8; manifest_len];
    file.read_exact(&mut manifest_bytes).map_err(|e| SpmError::invalid_format(format!("Failed to read manifest: {e}")))?;

    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| SpmError::invalid_format(format!("Failed to parse manifest: {e}")))?;
    Ok(manifest)
}

pub fn create_sam(
    manifest: &Manifest,
    data_path: &str,
    meta_path: Option<&str>,
    output_path: &str,
) -> SpmResult<()> {
    let mut output = fs::File::create(output_path)
        .map_err(|e| SpmError::other(format!("Failed to create .sam file {output_path}: {e}")))?;
    output.write_all(SAM_MAGIC)
        .map_err(|e| SpmError::other(format!("Failed to write magic: {e}")))?;

    let manifest_json = serde_json::to_vec(manifest)?;
    let manifest_len = (manifest_json.len() as u32).to_le_bytes();
    output.write_all(&manifest_len)?;
    output.write_all(&manifest_json)?;

    compress_to_zstd(data_path, &mut output)?;

    if let Some(meta) = meta_path {
        compress_to_zstd(meta, &mut output)?;
    }

    Ok(())
}

fn compress_to_zstd(input_path: &str, output: &mut fs::File) -> SpmResult<()> {
    let mut input = fs::File::open(input_path)?;
    let mut encoder = zstd::Encoder::new(output, 3)
        .map_err(|e| SpmError::compression(format!("Failed to create zstd encoder: {e}")))?;
    std::io::copy(&mut input, &mut encoder)
        .map_err(|e| SpmError::compression(format!("Failed to compress data: {e}")))?;
    encoder.finish()
        .map_err(|e| SpmError::compression(format!("Failed to finish zstd stream: {e}")))?;
    Ok(())
}

pub fn extract_sam(path: &str, target_dir: &str) -> SpmResult<(Manifest, Vec<String>)> {
    let mut file = fs::File::open(path)
        .map_err(|e| SpmError::invalid_format(format!("Failed to open .sam file {path}: {e}")))?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)
        .map_err(|e| SpmError::invalid_format(format!("Failed to read magic: {e}")))?;
    if &magic != SAM_MAGIC {
        return Err(SpmError::invalid_format("Invalid .sam file: bad magic"));
    }

    let mut len_buf = [0u8; 4];
    file.read_exact(&mut len_buf)?;
    let manifest_len = u32::from_le_bytes(len_buf) as usize;

    let mut manifest_bytes = vec![0u8; manifest_len];
    file.read_exact(&mut manifest_bytes)?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)?;

    let data_dir = Path::new(target_dir).join("data");
    fs::create_dir_all(&data_dir)?;

    let data_tar = data_dir.with_extension("tar");
    {
        let mut decoder = zstd::Decoder::new(&file)
            .map_err(|e| SpmError::compression(format!("Failed to create zstd decoder: {e}")))?;
        let mut data_file = fs::File::create(&data_tar)
            .map_err(|e| SpmError::other(format!("Failed to create data.tar: {e}")))?;
        std::io::copy(&mut decoder, &mut data_file)
            .map_err(|e| SpmError::compression(format!("Failed to decompress data: {e}")))?;
    }

    let mut extracted_files = Vec::new();
    let tar_file = fs::File::open(&data_tar)?;
    let mut archive = tar::Archive::new(tar_file);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let target_path = Path::new(target_dir).join(&path);
        crate::package::store::sanitize_relative(&target_path)?;
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&target_path)?;
        extracted_files.push(path.to_string_lossy().to_string());
    }

    let _ = fs::remove_file(&data_tar);

    Ok((manifest, extracted_files))
}

pub fn convert_to_sam(path: &str) -> SpmResult<()> {
    let pkg = crate::package::deb::parse_deb_control(path)
        .or_else(|_| crate::package::rpm::parse_rpm_header(path))?;

    let manifest = crate::types::Manifest {
        name: pkg.name,
        version: pkg.version,
        architecture: pkg.architecture,
        maintainer: pkg.maintainer,
        description: pkg.description,
        dependencies: pkg.dependencies,
        conflicts: pkg.conflicts,
        provides: pkg.provides,
        recommends: pkg.recommends,
        install_size: pkg.install_size,
        format_version: 1,
        source: Some(crate::types::PackageSource {
            original_format: format!("{:?}", pkg.format).to_lowercase(),
            original_package: std::path::Path::new(path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            repo: String::new(),
            hash_original: calculate_data_hash(path)?,
        }),
        ai_metadata: None,
        signature: None,
        systemd_units: vec![],
        sysusers: vec![],
        tmpfiles: vec![],
        triggers: vec![],
        obsoletes: vec![],
        conffiles: vec![],
    };

    let sam_path = format!("{}.sam", path);
    create_sam(&manifest, path, None, &sam_path)?;
    println!("Converted {} -> {}", path, sam_path);
    Ok(())
}

pub fn calculate_data_hash(path: &str) -> SpmResult<String> {
    let content = fs::read(path)?;
    Ok(hash::hash_bytes(&content))
}

/// Scan a .sam file for ELF SONAMEs using pure Rust ELF parsing.
/// Returns deduplicated SONAME strings found in the package's ELF files.
pub fn scan_sam_sonames(path: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("spm-elf-scan-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let result = (|| -> SpmResult<Vec<String>> {
        let (_manifest, files) = extract_sam(path, dir.to_str().ok_or_else(|| SpmError::other("bad tmpdir"))?)?;
        let mut sonames = Vec::new();
        for file in &files {
            let fpath = dir.join(file);
            if !fpath.is_file() {
                continue;
            }
            // Check ELF magic
            let mut magic = [0u8; 4];
            if fs::File::open(&fpath).and_then(|mut f| f.read_exact(&mut magic)).is_err() {
                continue;
            }
            if &magic != b"\x7fELF" {
                continue;
            }
            // Use goblin to extract SONAME from ELF dynamic section
            let bytes = match fs::read(&fpath) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if let Ok(binary) = goblin::elf::Elf::parse(&bytes) {
                if let Some(dynamic) = &binary.dynamic {
                    let strtab_offset = dynamic.info.strtab;
                    if strtab_offset > 0 && strtab_offset < bytes.len() {
                        let strtab_bytes = &bytes[strtab_offset..];
                        let strtab = goblin::strtab::Strtab::new(strtab_bytes, 0x0);
                        for entry in &dynamic.dyns {
                            if entry.d_tag == goblin::elf::dynamic::DT_SONAME {
                                if let Some(soname) = strtab.get_at(entry.d_val as usize) {
                                    let soname = soname.to_string();
                                    if !sonames.contains(&soname) {
                                        sonames.push(soname);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(sonames)
    })();
    let _ = fs::remove_dir_all(&dir);
    result.unwrap_or_default()
}

pub fn fetch_and_convert(name: &str, output_path: &Path) -> SpmResult<()> {
    let repos = crate::config::repos::load_repos()?;
    let cache_dir = crate::config::paths::repos_cache_dir();

    for (repo_name, config) in &repos {
        let repo_cache = cache_dir.join("native").join(repo_name).join("repo-index.json");
        if !repo_cache.exists() {
            continue;
        }
        let content = match fs::read_to_string(&repo_cache) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let index: crate::types::RepoIndex = match serde_json::from_str(&content) {
            Ok(i) => i,
            Err(_) => continue,
        };
        for pkg in &index.packages {
            if pkg.name == name {
                if let Some(repo_url) = &config.url {
                    let base = repo_url.trim_end_matches('/');
                    let dl_url = format!("{}/{}", base, pkg.filename);
                    let response = ureq::get(&dl_url)
                        .call()
                        .map_err(|e| SpmError::network(format!("Failed to download {dl_url}: {e}")))?;
                    let body = response.into_body()
                        .read_to_vec()
                        .map_err(|e| SpmError::network(format!("Read error: {e}")))?;
                    let actual_hash = crate::util::hash::hash_bytes(&body);
                    if actual_hash != pkg.hash {
                        return Err(SpmError::invalid_format(format!(
                            "Hash mismatch for '{name}': expected {}, got {}",
                            pkg.hash, actual_hash
                        )));
                    }
                    fs::write(output_path, &body)
                        .map_err(|e| SpmError::other(format!("Write error: {e}")))?;
                    return Ok(());
                }
            }
        }
    }

    Err(SpmError::PackageNotFound(name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_manifest() -> Manifest {
        Manifest {
            name: "test-pkg".into(),
            version: "1.0.0".into(),
            architecture: "amd64".into(),
            maintainer: "test@example.com".into(),
            description: "Test package for round-trip".into(),
            dependencies: vec![],
            conflicts: vec![],
            provides: vec![],
            recommends: vec![],
            install_size: 1024,
            format_version: 1,
            source: None,
            ai_metadata: None,
            signature: None,
            systemd_units: vec![],
            sysusers: vec![],
            tmpfiles: vec![],
            triggers: vec![],
            obsoletes: vec![],
            conffiles: vec![],
        }
    }

    fn create_test_data(dir: &std::path::Path) -> String {
        let src_dir = dir.join("src");
        std::fs::create_dir_all(src_dir.join("subdir")).unwrap();
        std::fs::write(src_dir.join("file1.txt"), b"content1").unwrap();
        std::fs::write(src_dir.join("file2.txt"), b"content2").unwrap();
        std::fs::write(src_dir.join("subdir").join("nested.txt"), b"nested").unwrap();

        let tar_path = dir.join("data.tar");
        let file = std::fs::File::create(&tar_path).unwrap();
        let mut builder = tar::Builder::new(file);
        builder.append_dir_all(".", &src_dir).unwrap();
        builder.finish().unwrap();
        drop(builder);

        tar_path.to_string_lossy().to_string()
    }

    fn create_test_meta(dir: &std::path::Path) -> String {
        let src_dir = dir.join("meta_src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("preinst.sh"), b"#!/bin/sh\necho preinst").unwrap();

        let tar_path = dir.join("meta.tar");
        let file = std::fs::File::create(&tar_path).unwrap();
        let mut builder = tar::Builder::new(file);
        builder.append_dir_all(".", &src_dir).unwrap();
        builder.finish().unwrap();
        drop(builder);

        tar_path.to_string_lossy().to_string()
    }

    #[test]
    fn test_create_sam_creates_file() {
        let dir = std::env::temp_dir().join("spm-sam-test-create");
        let data_path = create_test_data(&dir);
        let sam_path = dir.join("output.sam");

        let manifest = create_test_manifest();
        create_sam(&manifest, &data_path, None, sam_path.to_str().unwrap()).unwrap();

        assert!(sam_path.exists());
        let metadata = std::fs::metadata(&sam_path).unwrap();
        assert!(metadata.len() > 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_manifest_after_create() {
        let dir = std::env::temp_dir().join("spm-sam-test-manifest");
        let data_path = create_test_data(&dir);
        let sam_path = dir.join("output.sam");

        let manifest = create_test_manifest();
        create_sam(&manifest, &data_path, None, sam_path.to_str().unwrap()).unwrap();

        let read_manifest = read_manifest(sam_path.to_str().unwrap()).unwrap();
        assert_eq!(read_manifest.name, "test-pkg");
        assert_eq!(read_manifest.version, "1.0.0");
        assert_eq!(read_manifest.architecture, "amd64");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_sam_round_trip() {
        let dir = std::env::temp_dir().join("spm-sam-test-extract");
        let data_path = create_test_data(&dir);
        let sam_path = dir.join("output.sam");
        let extract_dir = dir.join("extracted");

        let manifest = create_test_manifest();
        create_sam(&manifest, &data_path, None, sam_path.to_str().unwrap()).unwrap();

        let (read_manifest, files) = extract_sam(sam_path.to_str().unwrap(), extract_dir.to_str().unwrap()).unwrap();

        assert_eq!(read_manifest.name, "test-pkg");
        assert!(files.contains(&"file1.txt".to_string()));
        assert!(files.contains(&"file2.txt".to_string()));
        assert!(files.contains(&"subdir/nested.txt".to_string()));

        let file1_path = extract_dir.join("file1.txt");
        assert!(file1_path.exists());
        assert_eq!(std::fs::read_to_string(file1_path).unwrap(), "content1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_sam_with_meta() {
        let dir = std::env::temp_dir().join("spm-sam-test-meta");
        let data_path = create_test_data(&dir);
        let meta_path = create_test_meta(&dir);

        let sam_path = dir.join("output.sam");

        let manifest = create_test_manifest();
        create_sam(&manifest, &data_path, Some(&meta_path), sam_path.to_str().unwrap()).unwrap();

        let (_manifest, files) = extract_sam(sam_path.to_str().unwrap(), dir.join("extracted").to_str().unwrap()).unwrap();
        assert!(files.contains(&"file1.txt".to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_invalid_magic_rejected() {
        let dir = std::env::temp_dir().join("spm-sam-test-bad-magic");
        std::fs::create_dir_all(&dir).unwrap();
        let bad_path = dir.join("bad.sam");
        std::fs::write(&bad_path, b"NOTSAM1").unwrap();

        let result = read_manifest(bad_path.to_str().unwrap());
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_empty_file_rejected() {
        let dir = std::env::temp_dir().join("spm-sam-test-empty");
        std::fs::create_dir_all(&dir).unwrap();
        let empty_path = dir.join("empty.sam");
        std::fs::write(&empty_path, b"").unwrap();

        let result = extract_sam(empty_path.to_str().unwrap(), dir.join("out").to_str().unwrap());
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_calculate_data_hash() {
        let dir = std::env::temp_dir().join("spm-sam-test-hash");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.bin");
        std::fs::write(&path, b"test data for hash").unwrap();

        let h = calculate_data_hash(path.to_str().unwrap()).unwrap();
        assert_eq!(h.len(), 64);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
