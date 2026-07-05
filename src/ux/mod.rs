pub mod completions;
pub mod formatter;
pub mod progress;
pub mod prompts;

use std::time::Duration;

/// Display info about a package shown during install prompts and summaries.
#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub source: String,
    pub size: u64,
    pub file_count: usize,
    pub sandboxed: bool,
}

/// Aggregate result handed back from an install operation.
#[derive(Debug, Clone)]
pub struct InstallResult {
    pub packages: Vec<PackageInfo>,
    pub duration: Duration,
    pub total_size: u64,
}

/// Foreign package managers that may own files spm wants to touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    Other(&'static str),
}

impl std::fmt::Display for PackageManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageManager::Apt => write!(f, "apt"),
            PackageManager::Dnf => write!(f, "dnf"),
            PackageManager::Pacman => write!(f, "pacman"),
            PackageManager::Zypper => write!(f, "zypper"),
            PackageManager::Other(s) => write!(f, "{}", s),
        }
    }
}

/// Perceived risk of a resolution option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    None,
    Low,
    Medium,
    High,
}

/// One selectable option shown during conflict resolution.
#[derive(Debug, Clone)]
pub struct ResolutionOption {
    pub description: String,
    pub command: String,
    pub risk: RiskLevel,
}

/// What to do when a foreign pm tries to write files owned by spm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignTouchAction {
    Allow,
    DenyAndKill,
    SandboxInstall,
}
