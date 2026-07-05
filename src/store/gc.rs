//! Garbage collection for the shared library store.
//!
//! A shared library (identified by its SONAME + BLAKE3 content hash) can be
//! collected when its reference count reaches zero **and** no providers
//! reference it.  The collector scans the [`SharedLibStore`], removes
//! eligible entries, deletes their backing files, and cleans up empty
//! SONAME directories.

use std::path::Path;

use crate::error::SpmResult;
use crate::store::deduplication::StoreManager;

/// Statistics returned by [`collect_garbage`].
#[derive(Debug, Default, Clone, Copy)]
pub struct GcStats {
    pub scanned: usize,
    pub removed: usize,
    pub errors: usize,
}

/// Run a full GC sweep over the shared library store.
///
/// Entries whose `ref_count` is zero and whose `providers` vector is empty
/// are removed from the indices and their backing file is deleted.  Empty
/// per-SONAME directories are removed as a best-effort clean-up.
pub fn collect_garbage(store: &mut StoreManager) -> SpmResult<GcStats> {
    let mut stats = GcStats::default();

    // Phase 1: identify collectable entries.
    let mut to_remove: Vec<String> = Vec::new();
    for (soname, entry) in store.shared_libs().iter() {
        stats.scanned += 1;
        if entry.ref_count() == 0 && entry.providers.is_empty() {
            to_remove.push(soname.clone());
        }
    }

    // Phase 2: remove backing files.
    for soname in &to_remove {
        if let Some(entry) = store.shared_libs().get(soname) {
            let shared_path = store.shared_libs().path_for(&entry.soname, &entry.hash);
            if shared_path.exists() {
                match std::fs::remove_file(&shared_path) {
                    Ok(_) => stats.removed += 1,
                    Err(e) => {
                        tracing::warn!("GC: failed to remove {}: {e}", shared_path.display());
                        stats.errors += 1;
                    }
                }
            }
        }
    }

    // Phase 3: remove from indices.
    for soname in &to_remove {
        store.shared_libs_mut().remove(soname);
        // Best-effort clean-up of empty SONAME directories.
        let dir = store.shared_libs().path().join(soname);
        remove_empty_dir(&dir);
    }

    Ok(stats)
}

/// Remove a single shared library by SONAME and hash.
///
/// Returns `true` if the entry existed and was removed, `false` otherwise.
pub fn remove_shared_lib(store: &mut StoreManager, soname: &str, hash: &str) -> SpmResult<bool> {
    let path = store.shared_libs().path_for(soname, hash);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let removed = store.shared_libs_mut().remove(soname).is_some();
    Ok(removed)
}

/// Remove an empty directory at `path`, ignoring errors.
fn remove_empty_dir(path: &Path) {
    if path.exists() {
        let _ = std::fs::remove_dir(path);
    }
}
