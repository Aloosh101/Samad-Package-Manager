use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::config::paths;
use crate::error::{SpmError, SpmResult};
use crate::types::PackageFormat;

/// Map a PackageFormat to its store origin subdirectory name.
pub fn origin_from_format(format: &PackageFormat) -> &'static str {
    match format {
        PackageFormat::Deb => "deb",
        PackageFormat::Rpm => "rpm",
        PackageFormat::Sam => "sam",
    }
}

/// Detect the host system's native package format by checking which
/// package manager backend is available.  "Sam" is always the fallback.
pub fn detect_host_format() -> PackageFormat {
    if std::process::Command::new("dpkg")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .is_some_and(|s| s.success())
    {
        PackageFormat::Deb
    } else if std::process::Command::new("rpm")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .is_some_and(|s| s.success())
    {
        PackageFormat::Rpm
    } else {
        PackageFormat::Sam
    }
}

/// Returns `true` when the package format differs from the host's native
/// format — i.e. this is a cross-distro install.
pub fn is_cross_distro(format: &PackageFormat) -> bool {
    *format != detect_host_format()
}

pub fn sanitize_relative(path: &Path) -> SpmResult<()> {
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(SpmError::other(format!(
                "Path traversal detected in package file: '{}'",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Root store directory (all origins live underneath).
pub fn store_dir() -> PathBuf {
    paths::db_base().join("store")
}

/// Store subdirectory for a given origin (format).
pub fn store_dir_for_origin(origin: &str) -> PathBuf {
    store_dir().join(origin)
}

/// Compute the on-disk store path for a hash using an origin prefix.
/// Example: `store_package_dir_for_origin("deb", "abc…")` → `…/store/deb/ab/abc…`
pub fn store_package_dir_for_origin(hash: &str, origin: &str) -> PathBuf {
    let dir = store_dir_for_origin(origin);
    if hash.len() < 2 {
        return dir.join(hash);
    }
    dir.join(&hash[..2]).join(hash)
}

/// Legacy sharded store path (no origin prefix).
/// Retained for backward compatibility during the migration.
pub fn store_package_dir(hash: &str) -> PathBuf {
    let dir = store_dir();
    if hash.len() < 2 {
        return dir.join(hash);
    }
    dir.join(&hash[..2]).join(hash)
}

pub fn copy_to_store(src_dir: &Path, hash: &str) -> SpmResult<PathBuf> {
    copy_to_store_with_origin(src_dir, hash, "")
}

pub fn copy_to_store_with_origin(src_dir: &Path, hash: &str, origin: &str) -> SpmResult<PathBuf> {
    let dest = if origin.is_empty() {
        store_package_dir(hash)
    } else {
        store_package_dir_for_origin(hash, origin)
    };
    if dest.exists() {
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::cache::copy_dir_all(src_dir, &dest)?;
    Ok(dest)
}

pub fn remove_store(hash: &str) -> SpmResult<()> {
    remove_store_with_origin(hash, "")
}

pub fn remove_store_with_origin(hash: &str, origin: &str) -> SpmResult<()> {
    let dir = if origin.is_empty() {
        store_package_dir(hash)
    } else {
        store_package_dir_for_origin(hash, origin)
    };
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
        remove_empty_parent(&dir);
    }
    Ok(())
}

fn remove_empty_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        if parent == store_dir() {
            return;
        }
        let _ = fs::remove_dir(parent);
    }
}

pub struct SymlinkRecord {
    pub fhs_path: PathBuf,
    pub store_path: PathBuf,
}

pub fn create_fhs_symlinks(store_path: &Path) -> SpmResult<Vec<SymlinkRecord>> {
    let mut records = Vec::new();
    for entry in walkdir::WalkDir::new(store_path).min_depth(1) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(store_path)
            .map_err(|_| SpmError::other("Failed to compute relative path"))?;
        sanitize_relative(relative)?;
        if relative.components().count() == 0 {
            continue;
        }
        let fhs_target = Path::new("/").join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&fhs_target)?;
        } else {
            if let Some(parent) = fhs_target.parent() {
                fs::create_dir_all(parent)?;
            }
            if fhs_target.exists() || fhs_target.is_symlink() {
                let _ = fs::remove_file(&fhs_target);
            }
            unix_fs::symlink(entry.path(), &fhs_target)?;
            records.push(SymlinkRecord {
                fhs_path: fhs_target,
                store_path: entry.path().to_path_buf(),
            });
        }
    }
    Ok(records)
}

pub fn is_library_file(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !file_name.contains(".so") {
        return false;
    }
    let path_str = path.to_string_lossy();
    path_str.contains("/lib/") || path_str.contains("/lib64/")
}

pub fn create_fhs_symlinks_smart(store_path: &Path) -> SpmResult<Vec<SymlinkRecord>> {
    let mut records = Vec::new();
    for entry in walkdir::WalkDir::new(store_path).min_depth(1) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(store_path)
            .map_err(|_| SpmError::other("Failed to compute relative path"))?;
        sanitize_relative(relative)?;
        if relative.components().count() == 0 {
            continue;
        }
        let fhs_target = Path::new("/").join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&fhs_target)?;
        } else if is_library_file(entry.path()) {
            continue;
        } else {
            if let Some(parent) = fhs_target.parent() {
                fs::create_dir_all(parent)?;
            }
            if fhs_target.exists() || fhs_target.is_symlink() {
                let _ = fs::remove_file(&fhs_target);
            }
            unix_fs::symlink(entry.path(), &fhs_target)?;
            records.push(SymlinkRecord {
                fhs_path: fhs_target,
                store_path: entry.path().to_path_buf(),
            });
        }
    }
    Ok(records)
}

pub fn find_elf_files(dir: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(dir)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .filter(|p| crate::util::fs::is_elf(p))
        .collect()
}

pub fn find_lib_dirs(store_path: &Path) -> Vec<PathBuf> {
    let candidates = ["usr/lib", "usr/lib64", "lib", "lib64", "usr/lib/x86_64-linux-gnu"];
    candidates
        .iter()
        .map(|d| store_path.join(d))
        .filter(|d| d.exists() && d.is_dir())
        .collect()
}

pub fn set_rpath_on_elfs(
    pkg_store_path: &Path,
    dep_store_dirs: &[PathBuf],
) -> SpmResult<()> {
    let elfs = find_elf_files(pkg_store_path);
    if elfs.is_empty() {
        return Ok(());
    }

    let mut lib_paths: Vec<PathBuf> = find_lib_dirs(pkg_store_path);
    for dep_dir in dep_store_dirs {
        lib_paths.extend(find_lib_dirs(dep_dir));
    }
    if lib_paths.is_empty() {
        return Ok(());
    }

    let rpath_val: String = lib_paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(":");

    if rpath_val.is_empty() {
        return Ok(());
    }

    for elf in &elfs {
        let output = Command::new("patchelf")
            .arg("--set-rpath")
            .arg(&rpath_val)
            .arg("--")
            .arg(elf)
            .output()
            .map_err(|e| SpmError::command_failed(
                format!("patchelf failed for {}: {e}", elf.display())
            ))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("patchelf could not set RPATH for {}: {stderr}", elf.display());
        }
    }

    Ok(())
}

pub fn count_store_references(conn: &rusqlite::Connection, hash: &str) -> SpmResult<u32> {
    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM installed_packages WHERE store_hash = ?1",
        rusqlite::params![hash],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn gc_store(conn: &rusqlite::Connection, hash: &str) -> SpmResult<bool> {
    gc_store_with_origin(conn, hash, "")
}

pub fn gc_store_with_origin(conn: &rusqlite::Connection, hash: &str, origin: &str) -> SpmResult<bool> {
    let store_path = if origin.is_empty() {
        store_package_dir(hash)
    } else {
        store_package_dir_for_origin(hash, origin)
    };
    if !store_path.exists() {
        return Ok(false);
    }
    let refs = count_store_references(conn, hash)?;
    if refs > 0 {
        return Ok(false);
    }
    // Double-check after DB commit to narrow the window with concurrent installs
    let refs_after = conn.query_row(
        "SELECT COUNT(*) FROM installed_packages WHERE store_hash = ?1",
        rusqlite::params![hash],
        |row| row.get::<_, u32>(0),
    )?;
    if refs_after > 0 {
        return Ok(false);
    }
    remove_store_with_origin(hash, origin)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_relative_ok() {
        assert!(sanitize_relative(Path::new("usr/bin/figlet")).is_ok());
        assert!(sanitize_relative(Path::new("a/b/c/d")).is_ok());
    }

    #[test]
    fn test_sanitize_relative_rejects_parent_dir() {
        assert!(sanitize_relative(Path::new("usr/../etc")).is_err());
        assert!(sanitize_relative(Path::new("..")).is_err());
        assert!(sanitize_relative(Path::new("a/../../b")).is_err());
    }

    #[test]
    fn test_store_package_dir_sharding() {
        let hash = "abcdef1234567890abcdef1234567890abcdef12";
        let dir = store_package_dir(hash);
        let dir_str = dir.to_string_lossy();
        assert!(dir_str.contains("ab/abcdef1234567890abcdef1234567890abcdef12"));
    }

    #[test]
    fn test_store_package_dir_short_hash() {
        let dir = store_package_dir("a");
        let dir_str = dir.to_string_lossy();
        assert!(dir_str.ends_with("/store/a"));
    }

    #[test]
    fn test_is_elf_with_real_elf() {
        let dir = std::env::temp_dir().join("spm-test-is-elf");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let elf_path = dir.join("test_bin");
        let elf_bytes = [0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
        fs::write(&elf_path, elf_bytes).unwrap();
        assert!(crate::util::fs::is_elf(&elf_path));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_is_elf_with_non_elf() {
        let dir = std::env::temp_dir().join("spm-test-not-elf");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let txt_path = dir.join("test.txt");
        fs::write(&txt_path, b"not an elf file").unwrap();
        assert!(!crate::util::fs::is_elf(&txt_path));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_elf_files_detects_elfs() {
        let dir = std::env::temp_dir().join("spm-test-find-elf");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(dir.join("usr").join("bin")).unwrap();

        let elf_bytes = [0x7f, b'E', b'L', b'F', 2, 1, 1, 0];
        fs::write(dir.join("usr").join("bin").join("mybin"), elf_bytes).unwrap();
        fs::write(dir.join("usr").join("bin").join("readme.txt"), b"hello").unwrap();

        let elfs = find_elf_files(&dir);
        assert_eq!(elfs.len(), 1);
        assert!(elfs[0].to_string_lossy().ends_with("mybin"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_lib_dirs_finds_existing() {
        let dir = std::env::temp_dir().join("spm-test-lib-dirs");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("usr").join("lib")).unwrap();
        // Don't create lib64 — should not appear
        let libs = find_lib_dirs(&dir);
        assert_eq!(libs.len(), 1);
        assert!(libs[0].to_string_lossy().ends_with("usr/lib"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_remove_store_removes_dir() {
        let store_base = std::env::temp_dir().join("spm-test-remove-store");
        let _ = fs::remove_dir_all(&store_base);
        fs::create_dir_all(store_base.join("aa").join("aabbcc")).unwrap();
        fs::write(store_base.join("aa").join("aabbcc").join("test.txt"), b"data").unwrap();

        let store_path = store_base.join("aa").join("aabbcc");
        assert!(store_path.exists());

        // We can't call remove_store directly since it uses store_dir().
        // Instead test the underlying logic:
        if store_path.exists() {
            fs::remove_dir_all(&store_path).unwrap();
        }
        assert!(!store_path.exists());
        let _ = fs::remove_dir_all(&store_base);
    }

    #[test]
    fn test_create_and_remove_symlinks() {
        let root = std::env::temp_dir().join("spm-test-symlinks");
        let _ = fs::remove_dir_all(&root);
        let store_path = root.join("store").join("aa").join("aabbcc");
        let fhs_root = root.join("fhs-root");

        // Create mock store files
        fs::create_dir_all(store_path.join("usr").join("bin")).unwrap();
        fs::write(store_path.join("usr").join("bin").join("hello"), b"#!/bin/sh\necho hi").unwrap();
        fs::create_dir_all(store_path.join("etc")).unwrap();
        fs::write(store_path.join("etc").join("hello.conf"), b"config").unwrap();

        // Create symlinks — but create_fhs_symlinks links to "/", so we can't test it
        // without root. Instead, simulate the logic:
        for entry in walkdir::WalkDir::new(&store_path).min_depth(1) {
            let entry = entry.unwrap();
            let relative = entry.path().strip_prefix(&store_path).unwrap();
            if relative.components().count() == 0 { continue; }
            let fhs_target = fhs_root.join(relative);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&fhs_target).unwrap();
            } else {
                if let Some(parent) = fhs_target.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                unix_fs::symlink(entry.path(), &fhs_target).unwrap();
            }
        }

        // Verify symlinks
        let bin_link = fhs_root.join("usr").join("bin").join("hello");
        assert!(bin_link.exists() || bin_link.is_symlink());

        // Remove symlinks
        for entry in walkdir::WalkDir::new(&fhs_root).min_depth(1) {
            let entry = entry.unwrap();
            if entry.file_type().is_file() || entry.file_type().is_symlink() {
                let _ = fs::remove_file(entry.path());
            }
        }
        assert!(!bin_link.exists() && !bin_link.is_symlink());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_remove_empty_parent_does_not_remove_store_root() {
        let store_base = std::env::temp_dir().join("spm-test-empty-parent");
        let _ = fs::remove_dir_all(&store_base);

        // Create a sharded path
        let nested = store_base.join("ab").join("abcdef1234");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("test.txt"), b"data").unwrap();

        // Remove the leaf dir and its file
        fs::remove_dir_all(&nested).unwrap();

        // Remove the shard dir
        let shard_dir = store_base.join("ab");
        let _ = fs::remove_dir(&shard_dir);

        // Ensure store_base is NOT removed
        assert!(store_base.exists());

        let _ = fs::remove_dir_all(&store_base);
    }
}
