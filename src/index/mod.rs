use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::SpmResult;
use crate::types::RepoSource;

pub mod deb;
pub mod rpm;
pub mod sam;

const SONAME_INDEX_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SonameIndex {
    pub version: u32,
    pub entries: HashMap<String, SonameEntry>,
}

impl Default for SonameIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl SonameIndex {
    pub fn new() -> Self {
        Self {
            version: SONAME_INDEX_VERSION,
            entries: HashMap::new(),
        }
    }

    pub fn insert_provider(&mut self, soname: &str, provider: SonameProvider) {
        let entry = self.entries.entry(soname.to_string()).or_insert_with(|| SonameEntry {
            requires: Vec::new(),
            providers: Vec::new(),
        });
        entry.providers.push(provider);
    }

    pub fn set_requires(&mut self, soname: &str, requires: Vec<String>) {
        let entry = self.entries.entry(soname.to_string()).or_insert_with(|| SonameEntry {
            requires: Vec::new(),
            providers: Vec::new(),
        });
        entry.requires = requires;
    }

    pub fn get_providers(&self, soname: &str) -> Option<&[SonameProvider]> {
        self.entries.get(soname).map(|e| e.providers.as_slice())
    }

    pub fn get_requires(&self, soname: &str) -> Option<&[String]> {
        self.entries.get(soname).map(|e| e.requires.as_slice())
    }

    pub fn load() -> SpmResult<Self> {
        let path = index_path();
        if !path.exists() {
            return Ok(Self::new());
        }
        let content = fs::read_to_string(&path)?;
        let index: SonameIndex = serde_json::from_str(&content)?;

        if index.version != SONAME_INDEX_VERSION {
            tracing::info!(
                "SONAME index version mismatch (found {}, expected {}), rebuilding...",
                index.version, SONAME_INDEX_VERSION
            );
            build_index()?;
            let content = fs::read_to_string(&path)?;
            return Ok(serde_json::from_str(&content)?);
        }

        Ok(index)
    }

    pub fn save(&self) -> SpmResult<()> {
        let path = index_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SonameEntry {
    pub requires: Vec<String>,
    pub providers: Vec<SonameProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SonameProvider {
    pub source: RepoSource,
    pub repo: String,
    pub pkg: String,
    pub version: String,
    pub priority: u32,
}

fn index_path() -> PathBuf {
    crate::config::paths::repos_cache_dir().join("soname-index.json")
}

pub fn build_index() -> SpmResult<()> {
    let mut index = SonameIndex::new();

    let repos = crate::config::repos::load_repos()?;
    for (repo_name, config) in &repos {
        match config.source {
            RepoSource::Deb => {
                if let Err(e) = deb::update_index(&mut index, repo_name, config) {
                    tracing::warn!("Failed to build SONAME index for apt repo '{}': {}", repo_name, e);
                }
            }
            RepoSource::Rpm => {
                if let Err(e) = rpm::update_index(&mut index, repo_name, config) {
                    tracing::warn!("Failed to build SONAME index for dnf repo '{}': {}", repo_name, e);
                }
            }
            RepoSource::Native => {
                if let Err(e) = sam::update_index(&mut index, repo_name, config) {
                    tracing::warn!("Failed to build SONAME index for native repo '{}': {}", repo_name, e);
                }
            }
        }
    }

    index.save()?;
    tracing::info!(
        "SONAME index built: {} entries",
        index.entries.len()
    );
    Ok(())
}
