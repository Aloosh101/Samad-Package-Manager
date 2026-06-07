use serde::{Deserialize, Serialize};

use super::package::PackageFormat;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub source: RepoSource,
    pub priority: Option<u32>,
    pub distro: Option<String>,
    pub codename: Option<String>,
    pub components: Option<Vec<String>>,
    pub mirrors: Option<Vec<String>>,
    pub release: Option<String>,
    pub repos: Option<Vec<String>>,
    pub url: Option<String>,
    /// Path to Ed25519 private key (PEM) for signing Release files
    pub signing_key: Option<String>,
}

impl RepoConfig {
    pub fn effective_priority(&self) -> u32 {
        self.priority.unwrap_or(100)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoSource {
    Apt,
    Dnf,
    Native,
}

impl std::fmt::Display for RepoSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoSource::Apt => write!(f, "apt"),
            RepoSource::Dnf => write!(f, "dnf"),
            RepoSource::Native => write!(f, "native"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpmConfig {
    pub db_path: Option<String>,
    pub cache_path: Option<String>,
    pub sandbox_path: Option<String>,
    pub log_level: Option<String>,
    pub auto_snapshot: Option<bool>,
    pub prefer_newest: Option<bool>,
    pub auto_update_interval: Option<u64>,
    /// Preferred repo source: "apt", "dnf", or "native"
    pub preferred_source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxLevel {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "standard")]
    Standard,
    #[serde(rename = "strict")]
    Strict,
    #[serde(rename = "full")]
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInstall {
    pub user_id: u32,
    pub package_name: String,
    pub package_format: PackageFormat,
    pub package_hash: String,
    pub installed_at: String,
}

/// Single package entry in a remote repo index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoIndexRecord {
    pub name: String,
    pub version: String,
    pub architecture: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub provides_soname: Vec<String>,
    pub conflicts: Vec<String>,
    pub filename: String,
    pub hash: String,
    pub size: u64,
}

/// The index file downloaded from a Native repo (`spm-repo.json`)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoIndex {
    pub repo_name: String,
    pub format_version: u32,
    pub packages: Vec<RepoIndexRecord>,
}
