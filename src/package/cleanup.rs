use std::collections::HashSet;
use std::path::Path;

use crate::config::paths;
use crate::db;
use crate::error::SpmResult;

pub fn cleanup_all(force: bool) -> SpmResult<()> {
    let mut cleaned = 0u64;
    let mut freed_bytes = 0u64;

    cleaned += purge_dead_symlinks(force)?;
    cleaned += gc_cache(force, &mut freed_bytes)?;
    cleaned += clean_temp_files()?;
    cleaned += clean_orphan_files(force)?;

    println!("Cleanup complete: removed {cleaned} items",);
    if freed_bytes > 0 {
        let mb = freed_bytes / (1024 * 1024);
        println!("Freed approximately {mb} MB",);
    }
    Ok(())
}

pub fn clean_orphan_files(force: bool) -> SpmResult<u64> {
    let scan_dirs = ["/etc", "/usr/lib", "/usr/share"];
    let conn = match db::get_connection() {
        Ok(c) => c,
        Err(_) => return Ok(0),
    };

    let mut check_stmt = match conn.prepare("SELECT 1 FROM files WHERE filepath = ?1 LIMIT 1") {
        Ok(s) => s,
        Err(_) => return Ok(0),
    };

    let mut count = 0u64;

    for dir in &scan_dirs {
        let base = Path::new(dir);
        if !base.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(base).min_depth(1).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let path_str = match path.to_str() {
                Some(s) => s,
                None => continue,
            };
            let owned = check_stmt.query_row(rusqlite::params![path_str], |_| Ok(()));
            if owned.is_ok() {
                continue;
            }
            if force {
                if std::fs::remove_file(path).is_ok() {
                    tracing::info!("Removed orphan file: {path_str}");
                    count += 1;
                }
            } else {
                println!("Orphan file: {path_str}");
                count += 1;
            }
        }
    }

    Ok(count)
}

pub fn purge_dead_symlinks(force: bool) -> SpmResult<u64> {
    let dirs = ["/usr/local/bin", "/usr/bin"];
    let mut count = 0u64;

    for dir in &dirs {
        let p = Path::new(dir);
        if !p.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(p) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_symlink() {
                let target = match std::fs::read_link(&path) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if !target.exists() {
                    if force {
                        if std::fs::remove_file(&path).is_ok() {
                            tracing::info!("Removed dead symlink: {:?} -> {:?}", path, target);
                            count += 1;
                        }
                    } else {
                        println!("Dead symlink: {:?} -> {:?}", path, target);
                    }
                }
            }
        }
    }

    Ok(count)
}

pub fn gc_cache(force: bool, freed: &mut u64) -> SpmResult<u64> {
    let cache_dir = paths::archives_dir();
    if !cache_dir.exists() {
        return Ok(0);
    }

    let conn = match db::get_connection() {
        Ok(c) => c,
        Err(_) => return Ok(0),
    };
    let installed = match db::list_installed_packages(&conn) {
        Ok(p) => p,
        Err(_) => return Ok(0),
    };

    let active_names: HashSet<String> = installed.iter().map(|p| p.name.clone()).collect();

    let entries = match std::fs::read_dir(&cache_dir) {
        Ok(e) => e,
        Err(_) => return Ok(0),
    };

    let mut count = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !active_names.contains(&name) {
            if force {
                if let Ok(meta) = std::fs::metadata(&path) {
                    *freed += meta.len();
                }
                if std::fs::remove_file(&path).is_ok() {
                    tracing::info!("GC: removed orphaned cache file: {:?}", path);
                    count += 1;
                }
            } else {
                let size_kb = std::fs::metadata(&path).map(|m| m.len() / 1024).unwrap_or(0);
                println!("Orphaned cache: {:?} ({} KB)", path, size_kb);
            }
        }
    }

    Ok(count)
}

pub fn clean_temp_files() -> SpmResult<u64> {
    let temp = std::env::temp_dir();
    let mut count = 0u64;

    let entries = match std::fs::read_dir(&temp) {
        Ok(e) => e,
        Err(_) => return Ok(0),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if name.starts_with("spm-extract-")
            || name.starts_with("spm-sandbox-")
            || name.starts_with("spm-trace-")
            || name.starts_with("spm-dl-")
        {
            if path.is_dir() {
                if std::fs::remove_dir_all(&path).is_ok() {
                    count += 1;
                }
            } else if std::fs::remove_file(&path).is_ok() {
                count += 1;
            }
        }
    }

    Ok(count)
}

pub fn gc_cache_for_package(name: &str) -> SpmResult<()> {
    let cache_dir = paths::archives_dir();
    if !cache_dir.exists() {
        return Ok(());
    }

    let entries = match std::fs::read_dir(&cache_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
            let matches = file_name == name
                || file_name == format!("{name}.deb")
                || file_name == format!("{name}.rpm")
                || file_name == format!("{name}.sam")
                || file_name.starts_with(&format!("{name}_"));
            if matches && std::fs::remove_file(&path).is_ok() {
                tracing::info!("Removed cached file for '{}': {:?}", name, path);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_clean_temp_files_empty() {
        // Should not fail when no spm temp files exist
        let result = clean_temp_files();
        assert!(result.is_ok());
    }

    #[test]
    fn test_clean_temp_files_with_spm_dirs() {
        let tmp = std::env::temp_dir();
        let dirs = [
            tmp.join("spm-extract-abc123"),
            tmp.join("spm-sandbox-test"),
            tmp.join("spm-dl-xyz"),
        ];
        for d in &dirs {
            let _ = fs::create_dir_all(d);
        }
        // Also create a non-spm file that should be ignored
        let other = tmp.join("spm-other-temp-file");
        let _ = fs::write(&other, b"data");

        let _ = clean_temp_files().unwrap();

        for d in &dirs {
            assert!(!d.exists(), "spm temp dir should have been removed");
        }
    }

    #[test]
    fn test_clean_temp_files_with_spm_files() {
        let tmp = std::env::temp_dir();
        let spm_file = tmp.join("spm-trace-clean-test.log");
        let _ = fs::write(&spm_file, b"trace data");

        let _ = clean_temp_files().unwrap();
        assert!(!spm_file.exists());
    }

    #[test]
    fn test_clean_temp_files_mixed() {
        let tmp = std::env::temp_dir();
        let spm_dirs = [
            tmp.join("spm-extract-mixed-1"),
            tmp.join("spm-sandbox-mixed-2"),
        ];
        let spm_files = [
            tmp.join("spm-dl-mixed.deb"),
            tmp.join("spm-trace-mixed.log"),
        ];
        let not_spm = tmp.join("keep-me.txt");
        for d in &spm_dirs {
            let _ = fs::create_dir_all(d);
        }
        for f in &spm_files {
            let _ = fs::write(f, b"data");
        }
        let _ = fs::write(&not_spm, b"keep");

        let _ = clean_temp_files().unwrap();

        for d in &spm_dirs {
            assert!(!d.exists());
        }
        for f in &spm_files {
            assert!(!f.exists());
        }
        assert!(not_spm.exists());
        let _ = fs::remove_file(&not_spm);
    }
}
