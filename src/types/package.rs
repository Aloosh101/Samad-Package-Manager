use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub architecture: String,
    pub maintainer: String,
    pub description: String,
    pub dependencies: Vec<Dependency>,
    pub conflicts: Vec<String>,
    pub provides: Vec<String>,
    pub recommends: Vec<String>,
    pub install_size: u64,
    pub format: PackageFormat,
    pub source_repo: Option<String>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageId {
    pub name: String,
    pub format: PackageFormat,
    pub version: String,
}

impl std::fmt::Display for PackageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.name, self.format)
    }
}

impl PackageId {
    pub fn new(name: &str, format: PackageFormat) -> Self {
        Self { name: name.to_string(), format, version: String::new() }
    }

    pub fn new_v(name: &str, format: PackageFormat, version: &str) -> Self {
        Self { name: name.to_string(), format, version: version.to_string() }
    }

    pub fn package_key(&self) -> String {
        self.to_string()
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        <Self as std::str::FromStr>::from_str(s).ok()
    }
}

impl std::str::FromStr for PackageId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.rsplit_once(':').and_then(|(name, fmt)| {
            let format = match fmt.to_lowercase().as_str() {
                "deb" => Some(PackageFormat::Deb),
                "rpm" => Some(PackageFormat::Rpm),
                "sam" => Some(PackageFormat::Sam),
                _ => None,
            }?;
            Some(PackageId { name: name.to_string(), format, version: String::new() })
        }).ok_or(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    pub source: DependencySource,
    pub format: Option<PackageFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DependencySource {
    System,
    Spm,
    Sandbox,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PackageFormat {
    #[default]
    Deb,
    Rpm,
    Sam,
}

impl std::fmt::Display for PackageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageFormat::Deb => write!(f, "deb"),
            PackageFormat::Rpm => write!(f, "rpm"),
            PackageFormat::Sam => write!(f, "sam"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub architecture: String,
    pub maintainer: String,
    pub description: String,
    pub dependencies: Vec<Dependency>,
    pub conflicts: Vec<String>,
    pub provides: Vec<String>,
    pub recommends: Vec<String>,
    pub install_size: u64,
    pub format_version: u32,
    pub source: Option<PackageSource>,
    pub ai_metadata: Option<AiMetadata>,
    pub signature: Option<PackageSignature>,
    // SAM v2 fields (optional, ignored by older spm versions)
    /// Systemd unit files shipped by this package (e.g. ["myapp.service"])
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub systemd_units: Vec<String>,
    /// System user/group definitions
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sysusers: Vec<SysuserEntry>,
    /// tmpfiles.d entries
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tmpfiles: Vec<TmpfileEntry>,
    /// Package trigger definitions (run on other package install/remove)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    /// Packages this package obsoletes (will be removed on install)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub obsoletes: Vec<String>,
    /// Configuration files shipped by this package.
    /// These are preserved on remove and backed up on upgrade when modified by the user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conffiles: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SysuserEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub entry_type: SysuserType,
    pub id: Option<String>,
    pub description: Option<String>,
    pub home: Option<String>,
    pub shell: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SysuserType {
    #[default]
    User,
    Group,
    Uuid,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TmpfileEntry {
    pub path: String,
    pub mode: String,
    pub uid: String,
    pub gid: String,
    pub age: Option<String>,
    pub argument: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Trigger {
    /// Trigger type: "install" | "remove" | "update"
    pub on: String,
    /// Package name pattern (glob) to trigger on
    pub pattern: String,
    /// Script to run (path relative to package root)
    pub script: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageSource {
    pub original_format: String,
    pub original_package: String,
    pub repo: String,
    pub hash_original: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiMetadata {
    pub converted: bool,
    pub conversion_date: String,
    pub dependencies_verified: bool,
    pub conflicts_resolved: Vec<String>,
    pub sandbox_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageSignature {
    pub algorithm: String,
    pub key_id: String,
    pub value: String,
}
