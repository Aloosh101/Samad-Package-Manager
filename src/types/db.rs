use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InstallOrigin {
    /// Installed by spm itself
    Spm,
    /// Found on system by spm sync (installed via dpkg/rpm directly)
    Foreign,
}

impl std::fmt::Display for InstallOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallOrigin::Spm => write!(f, "spm"),
            InstallOrigin::Foreign => write!(f, "foreign"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: Option<i64>,
    pub action: TransactionAction,
    pub timestamp: String,
    pub user: String,
    pub status: TransactionStatus,
    pub packages: Vec<String>,
    pub snapshot_id: Option<String>,
}

impl Transaction {
    pub fn package_ids(&self) -> Vec<super::PackageId> {
        self.packages.iter()
            .filter_map(|s| super::PackageId::from_str(s).or_else(|| {
                Some(super::PackageId { name: s.clone(), format: super::PackageFormat::Sam, version: String::new() })
            }))
            .collect()
    }

    pub fn add_package_id(&mut self, pid: &super::PackageId) {
        let s = pid.to_string();
        if !self.packages.contains(&s) {
            self.packages.push(s);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionAction {
    Install,
    Remove,
    Upgrade,
    Purge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionStatus {
    Completed,
    Undone,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: Option<i64>,
    pub transaction_id: i64,
    pub package: String,
    pub format: super::PackageFormat,
    pub filepath: String,
    pub hash: String,
    pub action: FileAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FileAction {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    pub format: super::PackageFormat,
    pub install_type: InstallType,
    pub manifest: Option<String>,
    pub install_date: String,
    pub source_repo: Option<String>,
    pub store_hash: Option<String>,
    pub origin: InstallOrigin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InstallType {
    Native,
    Sandbox,
}
