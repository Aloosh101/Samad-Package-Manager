use std::fs;
use std::path::{Path, PathBuf};

use crate::error::SpmResult;

pub struct DaemonStoreManager {
    store_path: PathBuf,
}

impl DaemonStoreManager {
    pub fn new() -> Self {
        Self {
            store_path: PathBuf::from("/var/lib/spm/store"),
        }
    }

    pub fn stage_package(
        &self,
        pkg_name: &str,
        format: &str,
        files: &[(PathBuf, Vec<u8>)],
    ) -> SpmResult<String> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(pkg_name.as_bytes());
        hasher.update(b"\0");
        hasher.update(format.as_bytes());
        hasher.update(b"\0");
        for (rel_path, content) in files {
            hasher.update(rel_path.to_string_lossy().as_bytes());
            hasher.update(b"\0");
            hasher.update(content);
        }
        let hash = hasher.finalize().to_hex().to_string();

        let pkg_dir = self.store_path.join(&hash);
        if pkg_dir.exists() {
            return Ok(hash);
        }
        fs::create_dir_all(&pkg_dir)?;

        for (rel_path, content) in files {
            let dest = pkg_dir.join(rel_path);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, content)?;
        }

        Ok(hash)
    }

    pub fn remove_package(&self, store_hash: &str) -> SpmResult<()> {
        let pkg_dir = self.store_path.join(store_hash);
        if pkg_dir.exists() {
            fs::remove_dir_all(&pkg_dir)?;
        }
        Ok(())
    }

    pub fn gc(&self) -> SpmResult<GcStats> {
        let mut files_removed = 0usize;
        let mut bytes_freed = 0u64;

        if !self.store_path.exists() {
            return Ok(GcStats {
                files_removed,
                bytes_freed,
            });
        }

        for entry in fs::read_dir(&self.store_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let size = dir_size(&path);
                if let Err(e) = fs::remove_dir_all(&path) {
                    tracing::warn!("GC: failed to remove {}: {e}", path.display());
                } else {
                    files_removed += 1;
                    bytes_freed += size;
                }
            }
        }

        Ok(GcStats {
            files_removed,
            bytes_freed,
        })
    }
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                total += meta.len();
                if meta.is_dir() {
                    total += dir_size(&entry.path());
                }
            }
        }
    }
    total
}

pub struct GcStats {
    pub files_removed: usize,
    pub bytes_freed: u64,
}
