use std::fs;
use std::io::Read;

use crate::error::{SpmError, SpmResult};
use crate::types::{Dependency, DependencySource, Package, PackageFormat};

const RPM_MAGIC: &[u8; 4] = b"\xed\xab\xee\xdb";

pub fn extract_rpm(path: &str, target: &str) -> SpmResult<()> {
    let target_path = std::path::Path::new(target);
    fs::create_dir_all(target_path)?;

    let mut file = fs::File::open(path)
        .map_err(|e| SpmError::invalid_format(format!("Failed to open .rpm file {path}: {e}")))?;
    let mut lead = [0u8; 96];
    file.read_exact(&mut lead)
        .map_err(|e| SpmError::invalid_format(format!("Failed to read RPM lead: {e}")))?;
    if &lead[0..4] != RPM_MAGIC {
        return Err(SpmError::invalid_format("Invalid .rpm file: bad magic"));
    }

    let _sig_data = read_index_data(&mut file, 16)?;
    let _hdr_data = read_index_data(&mut file, 16)?;

    let mut payload = Vec::new();
    file.read_to_end(&mut payload)?;

    let result = decompress_cpio(&payload, target_path);

    if result.is_ok() {
        return result;
    }

    tracing::debug!("Built-in CPIO parser failed, falling back to rpm2cpio: {}", result.unwrap_err());
    let status = std::process::Command::new("rpm2cpio")
        .arg(path)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| SpmError::command_failed(format!("Failed to spawn rpm2cpio: {e}")))
        .and_then(|child| {
            let output = std::process::Command::new("cpio")
                .args(["-idmv", "-D", target])
                .stdin(child.stdout.unwrap())
                .output()
                .map_err(|e| SpmError::command_failed(format!("Failed to run cpio: {e}")))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(SpmError::command_failed(format!("cpio failed: {stderr}")));
            }
            Ok(())
        });

    status
}

fn decompress_cpio(data: &[u8], target: &std::path::Path) -> SpmResult<()> {
    let decompressed = if data.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = flate2::read::GzDecoder::new(data);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out)?;
        out
    } else if data.len() > 4 {
        match zstd::decode_all(data) {
            Ok(out) => out,
            Err(_) => data.to_vec(),
        }
    } else {
        data.to_vec()
    };

    if decompressed.len() < 6 {
        return Ok(());
    }

    let mut offset = 0;
    while offset + 110 < decompressed.len() {
        let magic = &decompressed[offset..offset + 6];
        if magic != b"070701" && magic != b"070702" {
            break;
        }

        let read_u32 = |pos: usize| -> u32 {
            u32::from_be_bytes([
                decompressed[pos],
                decompressed[pos + 1],
                decompressed[pos + 2],
                decompressed[pos + 3],
            ])
        };

        let mode = read_u32(offset + 14);
        let namesize = read_u32(offset + 76);
        let filesize = read_u32(offset + 80);

        if namesize == 0 {
            break;
        }

        let name_offset = offset + 110;
        let name_end = name_offset + namesize as usize - 1;
        if name_end > decompressed.len() {
            break;
        }
        let name = String::from_utf8_lossy(&decompressed[name_offset..name_end]).to_string();

        let pad = (4 - (110 + namesize as usize) % 4) % 4;
        let data_offset = name_offset + namesize as usize + pad;
        let data_end = data_offset + filesize as usize;

        if data_end > decompressed.len() {
            break;
        }

        if !name.is_empty() && name != "." && name != "/" && name != "TRAILER!!!" {
            let clean_name = name.strip_prefix('/').unwrap_or(&name);
            let target_path = target.join(clean_name);
            crate::package::store::sanitize_relative(&target_path)?;

            if mode & 0o040_000 != 0 {
                fs::create_dir_all(&target_path)
                    .map_err(|e| SpmError::other(format!("Failed to create dir {:?}: {e}", target_path)))?;
            } else if filesize > 0 {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| SpmError::other(format!("Failed to create parent for {:?}: {e}", target_path)))?;
                }
                fs::write(&target_path, &decompressed[data_offset..data_end])
                    .map_err(|e| SpmError::other(format!("Failed to write {:?}: {e}", target_path)))?;
            }
        }

        let entry_size = 110 + namesize as usize + pad + filesize as usize;
        let entry_pad = (4 - entry_size % 4) % 4;
        offset += entry_size + entry_pad;

        if name == "TRAILER!!!" {
            break;
        }
    }

    Ok(())
}

fn read_index_data(file: &mut fs::File, _header_pad: u16) -> SpmResult<Vec<u8>> {
    let mut header = [0u8; 16];
    file.read_exact(&mut header).map_err(|e| SpmError::invalid_format(format!("Failed to read index header: {e}")))?;

    let nindex = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
    let hsize = u32::from_be_bytes([header[12], header[13], header[14], header[15]]);

    if nindex > 65536 || hsize > 4 * 1024 * 1024 {
        return Err(SpmError::invalid_format(
            format!("RPM header too large: {} entries, {} byte store", nindex, hsize)
        ));
    }

    let store_size = (nindex as usize * 16 + hsize as usize + 7) & !7;
    let mut store = vec![0u8; store_size];
    file.read_exact(&mut store).map_err(|e| SpmError::invalid_format(format!("Failed to read index data: {e}")))?;

    let mut result = header.to_vec();
    result.extend(store);
    Ok(result)
}

pub fn parse_rpm_header(path: &str) -> SpmResult<Package> {
    let mut file = fs::File::open(path).map_err(|e| SpmError::invalid_format(format!("Failed to open .rpm file {path}: {e}")))?;
    let mut lead = [0u8; 96];
    file.read_exact(&mut lead)?;
    if &lead[0..4] != RPM_MAGIC {
        return Err(SpmError::invalid_format("Invalid .rpm file: bad magic"));
    }

    let _sig_data = read_index_data(&mut file, 16)?;
    let hdr_data = read_index_data(&mut file, 16)?;

    let mut pkg = Package {
        name: String::new(),
        version: String::new(),
        architecture: String::new(),
        maintainer: String::new(),
        description: String::new(),
        dependencies: Vec::new(),
        conflicts: Vec::new(),
        provides: Vec::new(),
        recommends: Vec::new(),
        install_size: 0,
        format: PackageFormat::Rpm,
        source_repo: Some(path.to_string()),
    };

    let nindex = u32::from_be_bytes([hdr_data[8], hdr_data[9], hdr_data[10], hdr_data[11]]);
    let store_offset = 16 + (nindex as usize) * 16;

    let mut offset = 16usize;
    let mut tags: Vec<(i32, i32, i32, i32)> = Vec::new();
    for _ in 0..nindex {
        if offset + 16 > hdr_data.len() {
            break;
        }
        let tag = i32::from_be_bytes([
            hdr_data[offset],
            hdr_data[offset + 1],
            hdr_data[offset + 2],
            hdr_data[offset + 3],
        ]);
        let ty = i32::from_be_bytes([
            hdr_data[offset + 4],
            hdr_data[offset + 5],
            hdr_data[offset + 6],
            hdr_data[offset + 7],
        ]);
        let val_offset = i32::from_be_bytes([
            hdr_data[offset + 8],
            hdr_data[offset + 9],
            hdr_data[offset + 10],
            hdr_data[offset + 11],
        ]);
        let count = i32::from_be_bytes([
            hdr_data[offset + 12],
            hdr_data[offset + 13],
            hdr_data[offset + 14],
            hdr_data[offset + 15],
        ]);
        offset += 16;

        tags.push((tag, ty, val_offset, count));
    }

    let read_string_at = |store_off: i32| -> String {
        let start = store_offset + store_off as usize;
        if start >= hdr_data.len() {
            return String::new();
        }
        let end = hdr_data[start..]
            .iter()
            .position(|&b| b == 0)
            .map(|pos| start + pos)
            .unwrap_or(hdr_data.len());
        String::from_utf8_lossy(&hdr_data[start..end]).to_string()
    };

    let mut rpm_version = String::new();
    let mut rpm_release = String::new();
    let mut rpm_epoch: Option<i32> = None;

    for &(tag, ty, val_offset, count) in &tags {
        match tag {
            1000 if ty == 6 => pkg.name = read_string_at(val_offset),
            1001 if ty == 6 => rpm_version = read_string_at(val_offset),
            1002 if ty == 6 => rpm_release = read_string_at(val_offset),
            1003 if ty == 2 && count >= 1 => {
                let pos = store_offset + val_offset as usize;
                if pos + 4 <= hdr_data.len() {
                    rpm_epoch = Some(i32::from_be_bytes([
                        hdr_data[pos],
                        hdr_data[pos + 1],
                        hdr_data[pos + 2],
                        hdr_data[pos + 3],
                    ]));
                }
            }
            1004 if ty == 6 || ty == 8 || ty == 9 => {
                pkg.description = read_string_at(val_offset);
            }
            1005 if ty == 6 || ty == 8 || ty == 9 => {
                let s = read_string_at(val_offset);
                if !s.is_empty() {
                    pkg.description = s;
                }
            }
            1007 if ty == 6 => pkg.maintainer = read_string_at(val_offset),
            1022 if ty == 6 => pkg.architecture = read_string_at(val_offset),
            1009 if ty == 4 && count > 0 => {
                let pos = store_offset + val_offset as usize;
                if pos + 4 <= hdr_data.len() {
                    pkg.install_size = u32::from_be_bytes([
                        hdr_data[pos],
                        hdr_data[pos + 1],
                        hdr_data[pos + 2],
                        hdr_data[pos + 3],
                    ]) as u64;
                }
            }
            1049 if ty == 8 || ty == 9 => {
                let mut pos = store_offset + val_offset as usize;
                for _ in 0..count {
                    if pos >= hdr_data.len() {
                        break;
                    }
                    let end = hdr_data[pos..]
                        .iter()
                        .position(|&b| b == 0)
                        .map(|p| pos + p)
                        .unwrap_or(hdr_data.len());
                    let s = String::from_utf8_lossy(&hdr_data[pos..end]).to_string();
                    if !s.is_empty() && s != "()" && s != "(none)" && !s.starts_with("rpmlib(") && s != "python(abi)" {
                        pkg.dependencies.push(Dependency {
                            name: s,
                            version: String::new(),
                            source: DependencySource::Spm,
                            format: Some(PackageFormat::Rpm),
                        });
                    }
                    pos = end + 1;
                }
            }
            _ => {}
        }
    }

    // Combine epoch:version-release into version string
    let evr = match (rpm_epoch, rpm_release.as_str()) {
        (Some(e), rel) if !rel.is_empty() => format!("{}:{}-{}", e, rpm_version, rel),
        (Some(e), _) => format!("{}:{}", e, rpm_version),
        (None, rel) if !rel.is_empty() => format!("{}-{}", rpm_version, rel),
        (None, _) => rpm_version,
    };
    pkg.version = evr;

    Ok(pkg)
}

#[cfg(test)]
fn build_cpio_entry(magic: &[u8; 6], name: &str, data: &[u8], mode: u32) -> Vec<u8> {
    let namesize = name.len() + 1;
    let filesize = data.len();
    let pad_name = (4 - (110 + namesize) % 4) % 4;
    let pad_data = (4 - filesize % 4) % 4;
    let entry_size = 110 + namesize + pad_name + filesize + pad_data;

    let mut entry = vec![0u8; entry_size];
    entry[0..6].copy_from_slice(magic);
    // mode at offset 14 (parser reads u32 at offset+14)
    entry[14..18].copy_from_slice(&mode.to_be_bytes());
    // nlink at offset 30 (parser reads u32 at offset+30, not actually used)
    entry[30..34].copy_from_slice(&1u32.to_be_bytes());
    // namesize at offset 76 (parser reads u32 at offset+76)
    entry[76..80].copy_from_slice(&(namesize as u32).to_be_bytes());
    // filesize at offset 80 (parser reads u32 at offset+80)
    entry[80..84].copy_from_slice(&(filesize as u32).to_be_bytes());
    // filename at offset 110
    entry[110..110 + namesize - 1].copy_from_slice(name.as_bytes());
    entry[110 + namesize - 1] = 0;
    // data after padding
    let data_start = 110 + namesize + pad_name;
    entry[data_start..data_start + filesize].copy_from_slice(data);

    entry
}

#[cfg(test)]
fn build_cpio_trailer() -> Vec<u8> {
    let magic: &[u8; 6] = b"070701";
    build_cpio_entry(magic, "TRAILER!!!", b"", 0)
}

#[cfg(test)]
fn build_cpio_archive(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut archive = Vec::new();
    let magic: &[u8; 6] = b"070701";
    for (name, data, mode) in files {
        archive.extend(build_cpio_entry(magic, name, data, *mode));
    }
    archive.extend(build_cpio_trailer());
    archive
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decompress_cpio_empty() {
        let result = decompress_cpio(b"", std::path::Path::new("/tmp"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_decompress_cpio_too_short() {
        let result = decompress_cpio(b"short", std::path::Path::new("/tmp"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_decompress_cpio_invalid_magic() {
        let mut data = b"XXXXXX".to_vec();
        data.extend(std::iter::repeat_n(0u8, 110));
        let result = decompress_cpio(&data, std::path::Path::new("/tmp"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_decompress_cpio_file_extraction() {
        let dir = std::env::temp_dir().join("spm-rpm-test-extract");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let archive = build_cpio_archive(&[
            ("file.txt", b"hello world", 0o_100_644),
        ]);

        decompress_cpio(&archive, &dir).unwrap();
        let extracted = dir.join("file.txt");
        assert!(extracted.exists());
        assert_eq!(std::fs::read_to_string(extracted).unwrap(), "hello world");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_decompress_cpio_directory() {
        let dir = std::env::temp_dir().join("spm-rpm-test-dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let archive = build_cpio_archive(&[
            ("subdir", b"", 0o_040_755),
        ]);

        decompress_cpio(&archive, &dir).unwrap();
        assert!(dir.join("subdir").exists());
        assert!(dir.join("subdir").is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_decompress_cpio_nested_file() {
        let dir = std::env::temp_dir().join("spm-rpm-test-nested");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let archive = build_cpio_archive(&[
            ("usr", b"", 0o_040_755),
            ("usr/bin", b"", 0o_040_755),
            ("usr/bin/hello", b"#!/bin/sh\necho hello", 0o_100_755),
        ]);

        decompress_cpio(&archive, &dir).unwrap();
        assert!(dir.join("usr").exists());
        assert!(dir.join("usr/bin").exists());
        assert!(dir.join("usr/bin/hello").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("usr/bin/hello")).unwrap(),
            "#!/bin/sh\necho hello"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_decompress_cpio_strips_leading_slash() {
        let dir = std::env::temp_dir().join("spm-rpm-test-abs");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Build entry manually with leading slash in name
        let magic: &[u8; 6] = b"070701";
        let name = "/etc/nginx.conf";
        let data = b"server { }";
        let entry = build_cpio_entry(magic, name, data, 0o_100_644);
        let mut archive = Vec::new();
        archive.extend(&entry);
        archive.extend(build_cpio_trailer());

        decompress_cpio(&archive, &dir).unwrap();
        assert!(dir.join("etc").join("nginx.conf").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_decompress_cpio_070702_magic() {
        let dir = std::env::temp_dir().join("spm-rpm-test-070702");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Build entry with 070702 magic
        let magic: &[u8; 6] = b"070702";
        let name = "test.bin";
        let data = b"\x00\x01\x02\x03";
        let entry = build_cpio_entry(magic, name, data, 0o_100_644);
        let mut archive = Vec::new();
        archive.extend(&entry);
        archive.extend(build_cpio_trailer());

        decompress_cpio(&archive, &dir).unwrap();
        assert!(dir.join("test.bin").exists());
        assert_eq!(std::fs::read(dir.join("test.bin")).unwrap(), data);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_rpm_header_invalid_file() {
        let result = parse_rpm_header("/tmp/nonexistent-rpm-file-12345.rpm");
        assert!(result.is_err());
    }
}
