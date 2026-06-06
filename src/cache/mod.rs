use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::config::paths;
use crate::error::{SpmError, SpmResult};

pub fn packages_dir() -> PathBuf {
    paths::cache_dir().join("packages")
}

pub fn shared_files_dir() -> PathBuf {
    packages_dir().join("files")
}

pub fn init() -> SpmResult<()> {
    let dir = packages_dir();
    fs::create_dir_all(&dir)
        .map_err(|e| SpmError::other(format!("Cannot create packages dir {dir:?}: {e}")))?;

    let metadata = fs::metadata(&dir)
        .map_err(|e| SpmError::other(format!("Cannot stat packages dir {dir:?}: {e}")))?;
    let mut perms = metadata.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dir, perms)
        .map_err(|e| SpmError::other(format!("Cannot chmod packages dir {dir:?}: {e}")))?;

    fs::create_dir_all(shared_files_dir())
        .map_err(|e| SpmError::other(format!("Cannot create shared files dir: {e}")))?;

    Ok(())
}

pub fn cached_path(hash: &str) -> PathBuf {
    packages_dir().join(format!("{}.sam1", hash))
}

pub fn shared_file_path(hash: &str) -> PathBuf {
    let files_dir = shared_files_dir();
    if hash.len() < 2 {
        return files_dir.join(hash);
    }
    files_dir.join(&hash[..2]).join(hash)
}

/// Copy a file into the shared hash-addressed cache (no-op if already present)
pub fn store_shared_file(source: &Path, hash: &str) -> SpmResult<PathBuf> {
    let dest = shared_file_path(hash);
    if dest.exists() {
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| SpmError::other(format!("Cannot create shared dir {parent:?}: {e}")))?;
    }
    fs::copy(source, &dest)
        .map_err(|e| SpmError::other(format!("Cannot copy to shared cache {dest:?}: {e}")))?;
    Ok(dest)
}

/// Deploy a file from shared cache to a destination via hardlink (falls back to copy)
pub fn deploy_shared_file(hash: &str, dest: &Path) -> SpmResult<()> {
    let src = shared_file_path(hash);
    if !src.exists() {
        return Err(SpmError::other(format!(
            "Shared file not found in cache: {hash}"
        )));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| SpmError::other(format!("Cannot create deploy dir {parent:?}: {e}")))?;
    }
    fs::hard_link(&src, dest)
        .or_else(|_| fs::copy(&src, dest).map(|_| ()))
        .map_err(|e| SpmError::other(format!("Cannot deploy to {dest:?}: {e}")))?;
    Ok(())
}

pub fn packages_cache_dir() -> PathBuf {
    PathBuf::from("/var/spm/packages")
}

pub fn shared_package_dir(hash: &str) -> PathBuf {
    if hash.len() < 2 {
        packages_cache_dir().join(hash)
    } else {
        packages_cache_dir().join(&hash[..2]).join(hash)
    }
}

pub fn user_spm_dir(user_home: &str) -> PathBuf {
    PathBuf::from(user_home).join(".local").join("share").join("spm")
}

pub fn user_bin_dir(user_home: &str) -> PathBuf {
    user_spm_dir(user_home).join("bin")
}

pub fn user_lib_dir(user_home: &str) -> PathBuf {
    user_spm_dir(user_home).join("lib")
}

pub fn init_shared_cache() -> SpmResult<()> {
    let dir = packages_cache_dir();
    fs::create_dir_all(&dir)
        .map_err(|e| SpmError::other(format!("Cannot create shared cache dir {dir:?}: {e}")))?;
    fs::set_permissions(&dir, PermissionsExt::from_mode(0o755))
        .map_err(|e| SpmError::other(format!("Cannot chmod shared cache dir: {e}")))?;
    Ok(())
}

pub fn copy_to_shared_cache(source_dir: &Path, hash: &str) -> SpmResult<(PathBuf, bool)> {
    let dest_dir = shared_package_dir(hash);
    if dest_dir.exists() {
        return Ok((dest_dir, false));
    }
    if let Some(parent) = dest_dir.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| SpmError::other(format!("Cannot create shared cache parent {parent:?}: {e}")))?;
    }
    let tmp = dest_dir.with_extension("tmp");
    if tmp.exists() {
        let _ = fs::remove_dir_all(&tmp);
    }
    copy_dir_all(source_dir, &tmp)?;
    set_perms_recursive(&tmp, 0o755, 0o644)?;
    fs::rename(&tmp, &dest_dir)
        .map_err(|e| {
            let _ = fs::remove_dir_all(&tmp);
            SpmError::other(format!("Cannot rename to {dest_dir:?}: {e}"))
        })?;
    Ok((dest_dir, true))
}

fn set_perms_recursive(dir: &Path, dir_mode: u32, file_mode: u32) -> SpmResult<()> {
    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        if entry.file_type().is_dir() {
            let mut perms = meta.permissions();
            perms.set_mode(dir_mode);
            fs::set_permissions(entry.path(), perms)?;
        } else if entry.file_type().is_file() {
            let mut perms = meta.permissions();
            let was_exec = perms.mode() & 0o111 != 0;
            // Strip SUID/SGID/Sticky, set clean 755 for executables, 644 for others
            let clean_mode = if was_exec { 0o755 } else { file_mode };
            perms.set_mode(clean_mode);
            fs::set_permissions(entry.path(), perms)?;
        }
    }
    Ok(())
}

pub fn create_user_symlinks(hash: &str, user_home: &str) -> SpmResult<Vec<String>> {
    let pkg_dir = shared_package_dir(hash);
    if !pkg_dir.exists() {
        return Err(SpmError::other(format!(
            "Shared package not found in cache: {hash}"
        )));
    }
    let bin_dir = user_bin_dir(user_home);
    fs::create_dir_all(&bin_dir)?;

    let mut created = Vec::new();
    // Look for executables in common binary paths within the extracted package
    for subdir in &["usr/bin", "usr/local/bin", "bin"] {
        let src_dir = pkg_dir.join(subdir);
        if !src_dir.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&src_dir)? {
            let entry = entry?;
            let ftype = entry.file_type()?;
            if !ftype.is_file() && !ftype.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();
            let src = entry.path();
            let dest = bin_dir.join(&name_str);

            if dest.exists() {
                let _ = fs::remove_file(&dest);
            }
            std::os::unix::fs::symlink(&src, &dest)?;
            created.push(name_str);
        }
    }
    Ok(created)
}

pub fn remove_user_symlinks(user_home: &str, files: &[String]) -> SpmResult<()> {
    let bin_dir = user_bin_dir(user_home);
    if !bin_dir.exists() {
        return Ok(());
    }
    for file_name in files {
        let dest = bin_dir.join(file_name);
        if dest.exists() {
            let _ = fs::remove_file(&dest);
        }
    }
    Ok(())
}

pub fn remove_shared_package(hash: &str) -> SpmResult<()> {
    let dir = shared_package_dir(hash);
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
        // Clean up empty shard directories
        if let Some(parent) = dir.parent() {
            if parent.read_dir().map(|mut i| i.next().is_none()).unwrap_or(false) {
                let _ = fs::remove_dir(parent);
            }
        }
    }
    Ok(())
}

/// Remove a file from shared cache
pub fn remove_shared_file(hash: &str) -> SpmResult<()> {
    let path = shared_file_path(hash);
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|e| SpmError::other(format!("Cannot remove shared file {path:?}: {e}")))?;

        // Try to clean up empty shard directories (best-effort)
        if let Some(parent) = path.parent() {
            if parent.read_dir().map(|mut i| i.next().is_none()).unwrap_or(false) {
                let _ = fs::remove_dir(parent);
            }
        }
    }
    Ok(())
}

pub fn is_cached(hash: &str) -> bool {
    cached_path(hash).exists()
}

pub fn store_package(source: &Path, hash: &str) -> SpmResult<PathBuf> {
    init()?;
    let dest = cached_path(hash);
    if dest.exists() {
        return Ok(dest);
    }
    fs::copy(source, &dest)
        .map_err(|e| SpmError::other(format!("Cannot copy to cache {dest:?}: {e}")))?;
    Ok(dest)
}

pub fn verify_hash(path: &Path, expected_hash: &str) -> SpmResult<()> {
    let path_str = path.to_str().ok_or_else(|| SpmError::invalid_format(format!("Non-UTF-8 path: {:?}", path)))?;
    let computed = crate::util::hash::hash_file(path_str)?;
    if computed != expected_hash {
        return Err(SpmError::invalid_format(format!(
            "Hash mismatch for {:?}: expected {expected_hash}, computed {computed}",
            path
        )));
    }
    Ok(())
}

pub fn deploy_to_user(sam_path: &Path, _user_id: u32, user_home: &str) -> SpmResult<PathBuf> {
    let bin_dir = PathBuf::from(user_home).join(".local").join("share").join("spm").join("bin");
    fs::create_dir_all(&bin_dir)
        .map_err(|e| SpmError::other(format!("Cannot create {bin_dir:?}: {e}")))?;

    let extract_dir_obj = tempfile::tempdir()
        .map_err(|e| SpmError::other(format!("Failed to create temp directory: {e}")))?;
    let extract_dir = extract_dir_obj.path().to_path_buf();

    crate::package::sam::extract_sam(
        sam_path.to_str().unwrap_or_default(),
        extract_dir.to_str().unwrap_or_default(),
    )?;

    for entry in walkdir::WalkDir::new(&extract_dir).min_depth(1) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let relative = entry.path().strip_prefix(&extract_dir)
                .map_err(|e| SpmError::other(format!("Path error: {e}")))?;
            let dest = bin_dir.join(relative);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).ok();
            }
            fs::rename(entry.path(), &dest)
                .or_else(|_| fs::copy(entry.path(), &dest).map(|_| ()))
                .map_err(|e| SpmError::other(format!("Cannot deploy to {dest:?}: {e}")))?;

            let metadata = fs::metadata(&dest)
                .map_err(|e| SpmError::other(format!("Cannot stat {dest:?}: {e}")))?;
            let mut perms = metadata.permissions();
            let mode = perms.mode();
            if mode & 0o111 == 0 {
                perms.set_mode(mode | 0o755);
                fs::set_permissions(&dest, perms)
                    .map_err(|e| SpmError::other(format!("Cannot chmod {dest:?}: {e}")))?;
            }
        }
    }

    fs::remove_dir_all(&extract_dir).ok();
    Ok(bin_dir)
}

pub fn remove_user_binaries(hash: &str, user_home: &str) -> SpmResult<()> {
    let bin_dir = PathBuf::from(user_home).join(".local").join("share").join("spm").join("bin");
    if !bin_dir.exists() {
        return Ok(());
    }
    let cache_file = cached_path(hash);
    let extract_dir_obj = tempfile::tempdir()
        .map_err(|e| SpmError::other(format!("Failed to create temp directory: {e}")))?;
    let extract_dir = extract_dir_obj.path().to_path_buf();

    if cache_file.exists() {
        if let Some(sam_path) = cache_file.to_str() {
            if let Some(extract_path) = extract_dir.to_str() {
                let _ = crate::package::sam::extract_sam(sam_path, extract_path);
            }
        }
    }

    if extract_dir.exists() {
        for entry in walkdir::WalkDir::new(&extract_dir).min_depth(1).into_iter().flatten() {
            if entry.file_type().is_file() {
                if let Ok(relative) = entry.path().strip_prefix(&extract_dir) {
                    let dest = bin_dir.join(relative);
                    let _ = fs::remove_file(&dest);
                }
            }
        }
        fs::remove_dir_all(&extract_dir).ok();
    }

    Ok(())
}

pub fn gc(hash: &str) -> SpmResult<()> {
    let path = cached_path(hash);
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|e| SpmError::other(format!("GC: cannot remove {path:?}: {e}")))?;
        tracing::info!("GC: removed orphaned package {hash}");
    }
    Ok(())
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> SpmResult<()> {
    fs::create_dir_all(dst)?;
    for entry in walkdir::WalkDir::new(src).min_depth(1) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(src)
            .map_err(|e| SpmError::other(format!("Strip prefix error: {e}")))?;
        if relative.components().any(|c| c == std::path::Component::ParentDir) {
            return Err(SpmError::other(format!(
                "Path traversal detected: '{}'",
                relative.display()
            )));
        }
        let dest = dst.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest)?;
        } else if entry.file_type().is_file() || entry.file_type().is_symlink() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            if entry.file_type().is_symlink() {
                let target = fs::read_link(entry.path())?;
                std::os::unix::fs::symlink(&target, &dest)?;
            } else {
                fs::copy(entry.path(), &dest)?;
            }
        }
    }
    Ok(())
}

pub fn hash_file(path: &str) -> SpmResult<String> {
    let mut file = fs::File::open(path)
        .map_err(|e| SpmError::other(format!("Cannot open {path}: {e}")))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| SpmError::other(format!("Cannot read {path}: {e}")))?;
    let hash = blake3::hash(&buf);
    Ok(hash.to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_cached_path() {
        let p = cached_path("abc123");
        assert!(p.to_string_lossy().contains("abc123.sam1"));
    }

    #[test]
    fn test_shared_file_path_sharded() {
        let p = shared_file_path("abcdef123456");
        let s = p.to_string_lossy();
        assert!(s.contains("ab/abcdef123456"), "expected sharding, got {s}");
    }

    #[test]
    fn test_shared_file_path_short() {
        let p = shared_file_path("a");
        let s = p.to_string_lossy();
        // Short hashes (<2 chars) go directly, no sharding
        assert!(s.ends_with("/a"), "expected no sharding for short hash, got {s}");
    }

    #[test]
    fn test_shared_package_dir_sharded() {
        let p = shared_package_dir("abcdef123456");
        let s = p.to_string_lossy();
        assert!(s.contains("ab/abcdef123456"));
    }

    #[test]
    fn test_shared_package_dir_short() {
        let p = shared_package_dir("a");
        let s = p.to_string_lossy();
        assert!(s.ends_with("/a"));
    }

    #[test]
    fn test_user_spm_dir() {
        let d = user_spm_dir("/home/user");
        assert_eq!(d, PathBuf::from("/home/user/.local/share/spm"));
    }

    #[test]
    fn test_user_bin_dir() {
        let d = user_bin_dir("/home/user");
        assert_eq!(d, PathBuf::from("/home/user/.local/share/spm/bin"));
    }

    #[test]
    fn test_user_lib_dir() {
        let d = user_lib_dir("/home/user");
        assert_eq!(d, PathBuf::from("/home/user/.local/share/spm/lib"));
    }

    #[test]
    fn test_verify_hash_mismatch() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"hello").unwrap();
        let result = verify_hash(tmp.path(), "0000000000000000000000000000000000000000000000000000000000000000");
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("Hash mismatch"));
    }

    #[test]
    fn test_verify_hash_match() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"hello").unwrap();
        // Compute actual hash
        let actual = crate::util::hash::hash_file(tmp.path().to_str().unwrap()).unwrap();
        let result = verify_hash(tmp.path(), &actual);
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_cached_false() {
        assert!(!is_cached("nonexistent-hash-12345"));
    }

    #[test]
    fn test_packages_dir_is_dir() {
        // Just ensure it returns a PathBuf without panic
        let _ = packages_dir();
    }

    #[test]
    fn test_copy_dir_all_invalid_source() {
        let result = copy_dir_all(Path::new("/nonexistent-dir-12345"), Path::new("/tmp/spm-test-dest-12345"));
        assert!(result.is_err());
        // Cleanup
        let _ = std::fs::remove_dir_all("/tmp/spm-test-dest-12345");
    }

    #[test]
    fn test_store_shared_file_nonexistent_source() {
        let result = store_shared_file(Path::new("/nonexistent-file-12345"), "testhash123");
        assert!(result.is_err());
    }

    #[test]
    fn test_deploy_shared_file_nonexistent_hash() {
        let result = deploy_shared_file("nonexistent-hash-99999", Path::new("/tmp/spm-test-deploy-nonexistent"));
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_shared_file_nonexistent() {
        // Should not error
        let result = remove_shared_file("nonexistent-hash-for-remove-test");
        assert!(result.is_ok());
    }
}
