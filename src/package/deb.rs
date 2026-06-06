use std::fs;
use std::io::Read;
use std::path::Path;

use crate::error::{SpmError, SpmResult};
use crate::types::{Dependency, DependencySource, Package, PackageFormat};

const AR_MAGIC: &[u8; 8] = b"!<arch>\n";
const AR_HEADER_LEN: usize = 60;

struct ArHeader {
    name: String,
    size: usize,
}

fn read_ar_header(reader: &mut dyn Read) -> SpmResult<Option<ArHeader>> {
    let mut header = [0u8; AR_HEADER_LEN];
    match reader.read_exact(&mut header) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(SpmError::other(format!("Failed to read ar header: {e}"))),
    }

    let name_bytes = &header[0..16];
    let name = String::from_utf8_lossy(name_bytes).trim().to_string();
    let size_str = std::str::from_utf8(&header[48..58]).unwrap_or("0").trim();
    let size: usize = size_str.parse().unwrap_or(0);

    Ok(Some(ArHeader { name, size }))
}

fn read_ar_data(reader: &mut dyn Read, size: usize) -> SpmResult<Vec<u8>> {
    let mut data = vec![0u8; size];
    reader.read_exact(&mut data).map_err(|e| SpmError::other(format!("Failed to read ar data: {e}")))?;
    if size % 2 == 1 {
        let mut pad = [0u8; 1];
        reader.read_exact(&mut pad).map_err(|e| SpmError::other(format!("Failed to read ar padding: {e}")))?;
    }
    Ok(data)
}

fn normalize_ar_name(name: &str) -> &str {
    if let Some(end) = name.find('/') {
        name[..end].trim()
    } else {
        name.trim()
    }
}

pub fn extract_deb(path: &str, target: &str) -> SpmResult<()> {
    let backend = crate::util::backend::resolve("dpkg-deb");
    extract_deb_with(path, target, &backend)
        .or_else(|_| extract_deb_with(path, target, Path::new("dpkg-deb")))
}

fn extract_deb_with(path: &str, target: &str, cmd: &Path) -> SpmResult<()> {
    let output = std::process::Command::new(cmd)
        .args(["-x", path, target])
        .output()
        .map_err(|e| SpmError::command_failed(format!("Failed to run dpkg-deb: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SpmError::invalid_format(format!(
            "Failed to extract .deb '{}': {}",
            path,
            stderr.trim()
        )));
    }

    Ok(())
}

pub fn parse_deb_control(path: &str) -> SpmResult<Package> {
    let backend = crate::util::backend::resolve("dpkg-deb");
    parse_deb_control_with(path, &backend)
        .or_else(|_| parse_deb_control_with(path, Path::new("dpkg-deb")))
}

fn parse_deb_control_with(path: &str, cmd: &Path) -> SpmResult<Package> {
    let output = std::process::Command::new(cmd)
        .args(["-f", path])
        .output()
        .map_err(|e| SpmError::command_failed(format!("Failed to run dpkg-deb -f: {e}")))?;

    if output.status.success() {
        let control_content = String::from_utf8_lossy(&output.stdout);
        if !control_content.is_empty() {
            return parse_deb822(&control_content, path);
        }
    }

    parse_deb_control_fallback(path)
}

fn parse_deb_control_fallback(path: &str) -> SpmResult<Package> {
    let mut file = fs::File::open(path).map_err(|e| SpmError::invalid_format(format!("Failed to open .deb file {path}: {e}")))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).map_err(|e| SpmError::invalid_format(format!("Failed to read ar magic: {e}")))?;
    if &magic != AR_MAGIC {
        return Err(SpmError::invalid_format("Invalid .deb file: not an ar archive"));
    }

    let mut control_content = String::new();

    while let Some(header) = read_ar_header(&mut file)? {
        let name = normalize_ar_name(&header.name);
        let data = read_ar_data(&mut file, header.size)?;

        if name.starts_with("control.tar") {
            let mut decompressed = Vec::new();

            if let Ok(mut decoder) = zstd::Decoder::new(&data[..]) {
                decoder.read_to_end(&mut decompressed)?;
            } else {
                let mut decoder = flate2::read::GzDecoder::new(&data[..]);
                decoder.read_to_end(&mut decompressed)?;
            }

            if !decompressed.is_empty() {
                let mut archive = tar::Archive::new(&decompressed[..]);
                for entry in archive.entries()? {
                    let mut entry = entry?;
                    if let Ok(name) = entry.path().map(|p| p.to_string_lossy().to_string()) {
                        if name == "control" || name == "./control" {
                            entry.read_to_string(&mut control_content)?;
                        }
                    }
                }
            }
        }
    }

    if control_content.is_empty() {
        return Err(SpmError::invalid_format("No control file found in .deb"));
    }

    parse_deb822(&control_content, path)
}

fn parse_deb822(content: &str, source: &str) -> SpmResult<Package> {
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
        format: PackageFormat::Deb,
        source_repo: Some(source.to_string()),
    };

    let mut current_key = String::new();
    let mut current_value = String::new();

    for line in content.lines() {
        if line.is_empty() {
            current_key.clear();
            current_value.clear();
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            current_value.push('\n');
            current_value.push_str(line.trim());
        } else if let Some((key, val)) = line.split_once(':') {
            if !current_key.is_empty() {
                set_field(&mut pkg, &current_key, &current_value);
            }
            current_key = key.trim().to_string();
            current_value = val.trim().to_string();
        }
    }
    if !current_key.is_empty() {
        set_field(&mut pkg, &current_key, &current_value);
    }

    Ok(pkg)
}

fn set_field(pkg: &mut Package, key: &str, value: &str) {
    match key {
        "Package" => pkg.name = value.to_string(),
        "Version" => pkg.version = value.to_string(),
        "Architecture" => pkg.architecture = value.to_string(),
        "Maintainer" => pkg.maintainer = value.to_string(),
        "Description" => pkg.description = value.to_string(),
        "Installed-Size" => pkg.install_size = value.parse().unwrap_or(0) * 1024,
        "Depends" => pkg.dependencies = parse_depends(value),
        "Conflicts" => pkg.conflicts = value.split(',').map(|s| s.trim().to_string()).collect(),
        "Provides" => pkg.provides = value.split(',').map(|s| s.trim().to_string()).collect(),
        "Recommends" => pkg.recommends = value.split(',').map(|s| s.trim().to_string()).collect(),
        _ => {}
    }
}

fn parse_depends(value: &str) -> Vec<Dependency> {
    let mut result = Vec::new();
    for part in value.split(',') {
        let part = part.trim();
        // OR alternatives: only pick the first one (consistent with index/deb.rs:process_dep)
        let chosen = part.split('|').map(|s| s.trim()).find(|s| !s.is_empty()).unwrap_or("");
        if chosen.is_empty() {
            continue;
        }
        let (name, version) = if let Some((n, v)) = chosen.split_once('(') {
            (n.trim().to_string(), v.trim_matches(|c: char| c == ')' || c == ' ').to_string())
        } else {
            (chosen.to_string(), String::new())
        };
        result.push(Dependency {
            name,
            version,
            source: DependencySource::System,
            format: Some(PackageFormat::Deb),
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_deb822_basic() {
        let control = "Package: nginx\nVersion: 1.27.0\nArchitecture: amd64\nMaintainer: Test\nDescription: A test\n";
        let pkg = parse_deb822(control, "test.deb").unwrap();
        assert_eq!(pkg.name, "nginx");
        assert_eq!(pkg.version, "1.27.0");
        assert_eq!(pkg.architecture, "amd64");
        assert_eq!(pkg.maintainer, "Test");
        assert_eq!(pkg.description, "A test");
    }

    #[test]
    fn test_parse_deb822_with_fields() {
        let control = "Package: libssl3\nVersion: 3.0.15\nDepends: libc6 (>= 2.31), zlib1g\nConflicts: libssl1.1\nProvides: libssl\nRecommends: ca-certificates\nInstalled-Size: 5120\n";
        let pkg = parse_deb822(control, "test.deb").unwrap();
        assert_eq!(pkg.name, "libssl3");
        assert_eq!(pkg.dependencies.len(), 2);
        assert_eq!(pkg.dependencies[0].name, "libc6");
        assert_eq!(pkg.dependencies[0].version, ">= 2.31");
        assert_eq!(pkg.dependencies[1].name, "zlib1g");
        assert_eq!(pkg.conflicts, vec!["libssl1.1"]);
        assert_eq!(pkg.provides, vec!["libssl"]);
        assert_eq!(pkg.recommends, vec!["ca-certificates"]);
        assert_eq!(pkg.install_size, 5120 * 1024);
    }

    #[test]
    fn test_parse_deb822_multiline() {
        let control = "Package: nginx\nDescription: A web server\n It is very fast\n And reliable\n";
        let pkg = parse_deb822(control, "test.deb").unwrap();
        assert!(pkg.description.contains("web server"));
        assert!(pkg.description.contains("very fast"));
    }

    #[test]
    fn test_parse_depends_simple() {
        let deps = parse_depends("libc6, zlib1g");
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "libc6");
        assert_eq!(deps[1].name, "zlib1g");
    }

    #[test]
    fn test_parse_depends_with_versions() {
        let deps = parse_depends("libc6 (>= 2.31), libssl3 (>= 3.0.0)");
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "libc6");
        assert_eq!(deps[0].version, ">= 2.31");
        assert_eq!(deps[1].name, "libssl3");
        assert_eq!(deps[1].version, ">= 3.0.0");
    }

    #[test]
    fn test_parse_depends_empty() {
        let deps = parse_depends("");
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_depends_or_alternatives() {
        let deps = parse_depends("foo | bar");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "foo");
        // Only the first alternative is kept, consistent with index/deb.rs:process_dep
    }

    #[test]
    fn test_parse_depends_mixed_and_or() {
        let deps = parse_depends("libc6 (>= 2.31), foo | bar, baz");
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].name, "libc6");
        assert_eq!(deps[0].version, ">= 2.31");
        assert_eq!(deps[1].name, "foo");
        assert_eq!(deps[2].name, "baz");
    }

    #[test]
    fn test_set_field_valid() {
        let mut pkg = Package::default();
        set_field(&mut pkg, "Package", "mypkg");
        assert_eq!(pkg.name, "mypkg");
        set_field(&mut pkg, "Version", "2.0");
        assert_eq!(pkg.version, "2.0");
    }

    #[test]
    fn test_set_field_unknown() {
        let mut pkg = Package::default();
        set_field(&mut pkg, "Unknown-Field", "value");
        // Should not panic; field is ignored
        assert_eq!(pkg.name, "");
    }

    #[test]
    fn test_normalize_ar_name() {
        assert_eq!(normalize_ar_name("debian-binary/"), "debian-binary");
        assert_eq!(normalize_ar_name("control.tar.gz"), "control.tar.gz");
        assert_eq!(normalize_ar_name("data.tar.xz/   "), "data.tar.xz");
    }
}
