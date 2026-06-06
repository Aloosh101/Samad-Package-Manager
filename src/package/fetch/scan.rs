use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::error::SpmResult;
use crate::types::*;
use crate::util::hash;

pub(crate) fn scan_extracted_files(conn: &rusqlite::Connection, name: &str, extracted_dir: &str, _pkg: &InstalledPackage, _tmp_dir: &str) -> SpmResult<(Vec<FileRecord>, Vec<super::PackageConflict>)> {
    let base = Path::new(extracted_dir);
    if !base.exists() {
        return Ok((Vec::new(), Vec::new()));
    }

    let entries: Vec<_> = walkdir::WalkDir::new(base)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_type().is_dir())
        .collect();

    let mut records = Vec::with_capacity(entries.len());
    let mut conflicts = Vec::new();
    let owner_map = build_owner_map(conn);

    for entry in &entries {
        let relative = match entry.path().strip_prefix(base) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let target_str = format!("/{}", relative.display());
        let target_path = Path::new(&target_str);

        let file_hash = if entry.file_type().is_symlink() {
            match fs::read_link(entry.path()) {
                Ok(link) => hash::hash_bytes(link.to_string_lossy().as_bytes()),
                Err(_) => continue,
            }
        } else {
            match entry.path().to_str() {
                Some(s) => match hash::hash_file(s) {
                    Ok(h) => h,
                    Err(_) => continue,
                },
                None => continue,
            }
        };

        if target_path.exists() {
            if let Some(s) = target_path.to_str() {
                if let Ok(existing_hash) = hash::hash_file(s) {
                    if existing_hash != file_hash {
                        if let Ok(owner_pid) = lookup_owner(&target_str, &owner_map) {
                            let my_pid = crate::types::PackageId::new(name, _pkg.format.clone());
                            if owner_pid != my_pid {
                                conflicts.push(super::PackageConflict { owner: owner_pid.to_string() });
                            }
                        }
                    }
                }
            }
        }

        records.push(FileRecord {
            id: None,
            transaction_id: 0,
            package: name.to_string(),
            format: _pkg.format.clone(),
            filepath: target_str,
            hash: file_hash,
            action: FileAction::Created,
        });
    }

    Ok((records, conflicts))
}

fn build_owner_map(conn: &rusqlite::Connection) -> HashMap<String, crate::types::PackageId> {
    let mut map = HashMap::new();
    let query = "SELECT filepath, package, format FROM files";
    if let Ok(mut stmt) = conn.prepare(query) {
        if let Ok(rows) = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let pkg: String = row.get(1)?;
            let fmt_str: String = row.get(2)?;
            let fmt: crate::types::PackageFormat = serde_json::from_str(&fmt_str).unwrap_or_default();
            Ok((path, pkg, fmt))
        }) {
            for row in rows.flatten() {
                map.insert(row.0, crate::types::PackageId::new(&row.1, row.2));
            }
        }
    }
    map
}

fn lookup_owner(path: &str, map: &HashMap<String, crate::types::PackageId>) -> Result<crate::types::PackageId, ()> {
    map.get(path).cloned().ok_or(())
}

/// Detect conffiles in an extracted package directory.
/// Any file under `/etc/` is considered a conffile by convention.
pub(crate) fn detect_conffiles(extracted_dir: &str) -> Vec<String> {
    let base = Path::new(extracted_dir);
    let etc = base.join("etc");
    if !etc.exists() || !etc.is_dir() {
        return Vec::new();
    }

    let mut conffiles = Vec::new();
    for entry in walkdir::WalkDir::new(&etc).min_depth(1).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_dir() {
            continue;
        }
        if let Ok(relative) = entry.path().strip_prefix(base) {
            conffiles.push(format!("/{}", relative.display()));
        }
    }
    conffiles
}

pub(crate) fn dir_size(path: &Path) -> u64 {
    fn inner(path: &Path) -> std::io::Result<u64> {
        let mut total = 0u64;
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                total += inner(&entry.path())?;
            }
        } else if path.is_file() {
            total += fs::metadata(path)?.len();
        }
        Ok(total)
    }
    inner(path).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::types::PackageId;

    #[test]
    fn test_lookup_owner_found() {
        let mut map = HashMap::new();
        let pid = PackageId::new("nginx", crate::types::PackageFormat::Deb);
        map.insert("/usr/bin/nginx".to_string(), pid.clone());
        let result = lookup_owner("/usr/bin/nginx", &map);
        assert_eq!(result, Ok(pid));
    }

    #[test]
    fn test_lookup_owner_not_found() {
        let map = HashMap::new();
        let result = lookup_owner("/usr/bin/nginx", &map);
        assert_eq!(result, Err(()));
    }

    #[test]
    fn test_dir_size_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(dir_size(dir.path()), 0);
    }

    #[test]
    fn test_dir_size_with_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("test.txt");
        std::fs::write(&f, b"hello").unwrap();
        assert_eq!(dir_size(dir.path()), 5);
    }

    #[test]
    fn test_dir_size_nonexistent() {
        assert_eq!(dir_size(Path::new("/nonexistent-path-xyz")), 0);
    }

    #[test]
    fn test_dir_size_nested_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("a.txt"), b"abc").unwrap();
        std::fs::write(dir.path().join("b.txt"), b"de").unwrap();
        assert_eq!(dir_size(dir.path()), 5);
    }

    #[test]
    fn test_detect_conffiles_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = detect_conffiles(dir.path().to_str().unwrap());
        assert!(result.is_empty());
    }

    #[test]
    fn test_detect_conffiles_with_etc() {
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().join("etc/nginx");
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::write(etc.join("nginx.conf"), b"server {}").unwrap();
        std::fs::write(etc.join("mime.types"), b"types {}").unwrap();
        let result = detect_conffiles(dir.path().to_str().unwrap());
        assert!(result.contains(&"/etc/nginx/nginx.conf".to_string()));
        assert!(result.contains(&"/etc/nginx/mime.types".to_string()));
    }

    #[test]
    fn test_detect_conffiles_no_etc() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("usr/bin")).unwrap();
        std::fs::write(dir.path().join("usr/bin/myapp"), b"binary").unwrap();
        let result = detect_conffiles(dir.path().to_str().unwrap());
        assert!(result.is_empty());
    }
}
