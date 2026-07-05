use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::types::PackageId;

const DEFAULT_TTL_SECS: u64 = 300;
const CACHE_EPOCH_KEY: &str = "__epoch__";

struct CacheEntry {
    packages: Vec<PackageId>,
    inserted_at: Instant,
}

pub struct DependencyCache {
    epoch: u64,
    ttl: Duration,
    entries: HashMap<String, CacheEntry>,
}

impl DependencyCache {
    pub fn new() -> Self {
        Self {
            epoch: 0,
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
            entries: HashMap::new(),
        }
    }

    pub fn with_ttl(ttl_secs: u64) -> Self {
        Self {
            epoch: 0,
            ttl: Duration::from_secs(ttl_secs),
            entries: HashMap::new(),
        }
    }

    fn cache_key(root: &PackageId, strategy: u8) -> String {
        format!("{}:{}:{}:e{}", root.name, root.format, root.version, strategy)
    }

    pub fn get(&self, root: &PackageId, strategy: u8) -> Option<Vec<PackageId>> {
        let key = Self::cache_key(root, strategy);
        let entry = self.entries.get(&key)?;
        if entry.inserted_at.elapsed() > self.ttl {
            return None;
        }
        Some(entry.packages.clone())
    }

    pub fn set(&mut self, root: &PackageId, strategy: u8, packages: Vec<PackageId>) {
        let key = Self::cache_key(root, strategy);
        self.entries.insert(
            key,
            CacheEntry {
                packages,
                inserted_at: Instant::now(),
            },
        );
    }

    pub fn invalidate_all(&mut self) {
        self.epoch += 1;
        self.entries.clear();
    }

    pub fn current_epoch(&self) -> u64 {
        self.epoch
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bump_epoch(&mut self) {
        self.invalidate_all();
    }
}

impl Default for DependencyCache {
    fn default() -> Self {
        Self::new()
    }
}

static mut GLOBAL_CACHE_EPOCH: u64 = 0;

pub fn bump_dep_cache_epoch() {
    unsafe {
        GLOBAL_CACHE_EPOCH = GLOBAL_CACHE_EPOCH.wrapping_add(1);
    }
}

pub fn current_dep_cache_epoch() -> u64 {
    unsafe { GLOBAL_CACHE_EPOCH }
}
