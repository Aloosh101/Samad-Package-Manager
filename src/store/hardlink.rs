//! Hardlink management for content-addressed shared library deduplication.
//!
//! The main entry point is [`deduplicate_library`], which BLAKE3-hashes a
//! `.so` file, checks [`SharedLibStore`] for a matching hash, and either
//! creates a hardlink (if the content is already stored) or copies the file
//! into the shared store and links back into the origin store.

use std::sync::atomic::Ordering;

use crate::error::SpmResult;
use crate::store::deduplication::{
    extract_pkg_name, hardlink_or_copy, priority_for_format, Provider, SharedLibEntry, StoreManager,
};
use crate::types::PackageFormat;

/// Deduplicate a shared library at `lib_path` during package extraction.
///
/// 1. Computes the BLAKE3 hash of `lib_path`.
/// 2. If that hash already exists in [`StoreManager::shared_libs`], a
///    hardlink is created and the provider / ref-count is updated.
/// 3. Otherwise the file is copied into the shared store, a hardlink is
///    placed back at `lib_path`, and a new [`SharedLibEntry`] is inserted.
pub fn deduplicate_library(
    store: &mut StoreManager,
    origin_format: PackageFormat,
    origin_hash: &str,
    lib_path: &std::path::Path,
    soname: &str,
) -> SpmResult<()> {
    let hash = crate::store::deduplication::blake3_hash_file(lib_path)?;
    let hash_hex = hash.to_string();

    if let Some(entry) = store.shared_libs().get_by_hash(&hash_hex) {
        // Content already exists in the shared store.
        let shared_path = store.shared_libs().path_for(&entry.soname, &hash_hex);
        hardlink_or_copy(lib_path, &shared_path)?;

        if let Some(entry_mut) = store.shared_libs_mut().get_by_hash_mut(&hash_hex) {
            entry_mut.add_provider(Provider {
                origin: origin_format.clone(),
                package_name: extract_pkg_name(origin_hash),
                package_hash: origin_hash.to_string(),
                priority: priority_for_format(&origin_format),
            });
            entry_mut.ref_count.fetch_add(1, Ordering::Relaxed);
        }

        std::fs::remove_file(lib_path)?;
        std::fs::hard_link(&shared_path, lib_path)?;
        return Ok(());
    }

    // New content – copy into the shared store.
    let shared_path = store.shared_libs().path_for(soname, &hash_hex);
    if let Some(parent) = shared_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(lib_path, &shared_path)?;

    std::fs::remove_file(lib_path)?;
    std::fs::hard_link(&shared_path, lib_path)?;

    let mut entry = SharedLibEntry::new(soname.to_string(), hash_hex);
    entry.add_provider(Provider {
        origin: origin_format.clone(),
        package_name: extract_pkg_name(origin_hash),
        package_hash: origin_hash.to_string(),
        priority: priority_for_format(&origin_format),
    });
    entry.ref_count.fetch_add(1, Ordering::Relaxed);

    store.shared_libs_mut().insert(entry);

    Ok(())
}
