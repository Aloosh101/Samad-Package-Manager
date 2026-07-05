//! Content-addressed store management and shared library deduplication.
//!
//! [`StoreManager`] manages per-format origin stores (`deb`, `rpm`, `sam`)
//! and a unified [`SharedLibStore`] for SONAME-indexed shared libraries.
//! When a package is extracted, each `.so` file is BLAKE3-hashed; matching
//! hashes produce hardlinks instead of duplicate copies, while new content
//! is stored once and tracked by SONAME + hash.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::types::PackageFormat;

// ---------------------------------------------------------------------------
// StoreManager
// ---------------------------------------------------------------------------

/// Top-level store manager holding per-format origin stores and the shared
/// library store.
pub struct StoreManager {
    base: PathBuf,
    origins: HashMap<PackageFormat, OriginStore>,
    shared_libs: SharedLibStore,
}

impl StoreManager {
    /// Create a new `StoreManager` rooted at `base`.
    ///
    /// Origin stores are created under `base/{deb,rpm,sam}/` and the shared
    /// library store lives at `base/shared/libs/`.
    pub fn new(base: PathBuf) -> Self {
        let shared_libs_path = base.join("shared").join("libs");
        let mut origins = HashMap::new();
        for fmt in &[PackageFormat::Deb, PackageFormat::Rpm, PackageFormat::Sam] {
            let path = base.join(fmt.to_string());
            origins.insert(
                fmt.clone(),
                OriginStore {
                    format: fmt.clone(),
                    path,
                },
            );
        }
        Self {
            base,
            origins,
            shared_libs: SharedLibStore::new(shared_libs_path),
        }
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    pub fn origin(&self, fmt: &PackageFormat) -> Option<&OriginStore> {
        self.origins.get(fmt)
    }

    pub fn shared_libs(&self) -> &SharedLibStore {
        &self.shared_libs
    }

    pub fn shared_libs_mut(&mut self) -> &mut SharedLibStore {
        &mut self.shared_libs
    }
}

// ---------------------------------------------------------------------------
// OriginStore
// ---------------------------------------------------------------------------

/// A per-format origin directory (e.g. `/var/lib/spm/store/deb/`).
pub struct OriginStore {
    format: PackageFormat,
    path: PathBuf,
}

impl OriginStore {
    pub fn format(&self) -> &PackageFormat {
        &self.format
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// SharedLibStore
// ---------------------------------------------------------------------------

/// SONAME-indexed, content-addressed store for shared libraries.
///
/// Every unique library is stored at `path/{soname}/{hash}`.  The
/// `soname_index` maps SONAME → [`SharedLibEntry`] and the `hash_index`
/// maps content hash → SONAME for O(1) hash lookups.
pub struct SharedLibStore {
    path: PathBuf,
    pub(crate) soname_index: BTreeMap<String, SharedLibEntry>,
    pub(crate) hash_index: HashMap<String, String>,
}

impl SharedLibStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            soname_index: BTreeMap::new(),
            hash_index: HashMap::new(),
        }
    }

    /// The base path of the shared library store.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// On-disk path for a library identified by `soname` and content `hash`.
    pub fn path_for(&self, soname: &str, hash: &str) -> PathBuf {
        self.path.join(soname).join(hash)
    }

    /// Look up an entry by SONAME.
    pub fn get(&self, soname: &str) -> Option<&SharedLibEntry> {
        self.soname_index.get(soname)
    }

    /// Look up an entry by content hash (hex-encoded BLAKE3).
    pub fn get_by_hash(&self, hash: &str) -> Option<&SharedLibEntry> {
        self.hash_index
            .get(hash)
            .and_then(|soname| self.soname_index.get(soname))
    }

    /// Mutable lookup by content hash.
    pub fn get_by_hash_mut(&mut self, hash: &str) -> Option<&mut SharedLibEntry> {
        let soname = self.hash_index.get(hash)?.clone();
        self.soname_index.get_mut(&soname)
    }

    /// Insert a new shared library entry.
    pub fn insert(&mut self, entry: SharedLibEntry) {
        let hash = entry.hash.clone();
        let soname = entry.soname.clone();
        self.hash_index.insert(hash, soname.clone());
        self.soname_index.insert(soname, entry);
    }

    /// Remove an entry by SONAME, returning it if it existed.
    pub fn remove(&mut self, soname: &str) -> Option<SharedLibEntry> {
        if let Some(entry) = self.soname_index.remove(soname) {
            self.hash_index.remove(&entry.hash);
            Some(entry)
        } else {
            None
        }
    }

    /// Iterate over all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &SharedLibEntry)> {
        self.soname_index.iter()
    }
}

// ---------------------------------------------------------------------------
// SharedLibEntry
// ---------------------------------------------------------------------------

/// Metadata for a single content-addressed shared library.
///
/// `ref_count` is wrapped in [`Arc`] so the entry remains [`Clone`] while
/// sharing the reference count across logical copies.
#[derive(Clone)]
pub struct SharedLibEntry {
    pub soname: String,
    pub hash: String,
    pub providers: Vec<Provider>,
    pub ref_count: Arc<AtomicUsize>,
}

impl SharedLibEntry {
    pub fn new(soname: String, hash: String) -> Self {
        Self {
            soname,
            hash,
            providers: Vec::new(),
            ref_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Add a provider and bump the reference count.
    pub fn add_provider(&mut self, provider: Provider) {
        self.providers.push(provider);
    }

    /// Current reference count.
    pub fn ref_count(&self) -> usize {
        self.ref_count.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Records which package (from which format origin) provides a given SONAME.
#[derive(Clone, Debug)]
pub struct Provider {
    pub origin: PackageFormat,
    pub package_name: String,
    pub package_hash: String,
    pub priority: u32,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a human-readable package name from an origin hash.
pub fn extract_pkg_name(origin_hash: &str) -> String {
    origin_hash
        .split('-')
        .next()
        .unwrap_or(origin_hash)
        .to_string()
}

/// Priority value for a given `PackageFormat` (higher = preferred).
pub fn priority_for_format(fmt: &PackageFormat) -> u32 {
    match fmt {
        PackageFormat::Sam => 100,
        PackageFormat::Deb => 50,
        PackageFormat::Rpm => 50,
    }
}

/// BLAKE3 hash the contents of a file.
pub fn blake3_hash_file(path: &Path) -> crate::error::SpmResult<blake3::Hash> {
    use std::fs::File;
    use std::io::Read;

    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize())
}

/// Create a hardlink at `dst` pointing to `src`, falling back to copy when
/// the filesystem does not support cross-device hardlinks.
pub fn hardlink_or_copy(src: &Path, dst: &Path) -> crate::error::SpmResult<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::hard_link(src, dst).or_else(|_| {
        std::fs::copy(src, dst)?;
        Ok(())
    })
}
