use thiserror::Error;

#[derive(Error, Debug)]
pub enum SpmError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("TOML write error: {0}")]
    TomlSerialize(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Directory walk error: {0}")]
    Walkdir(#[from] walkdir::Error),

    #[error("Compression error: {0}")]
    Compression(String),

    #[error("Package not found: {0}")]
    PackageNotFound(String),

    #[error("Package already installed: {0}")]
    PackageAlreadyInstalled(String),

    #[error("Dependency resolution failed: {0}")]
    DependencyFailed(String),

    #[error("Invalid package format: {0}")]
    InvalidFormat(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Sandbox error: {0}")]
    Sandbox(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Command failed: {0}")]
    CommandFailed(String),

    #[error("{0}")]
    Other(String),
}

pub type SpmResult<T> = Result<T, SpmError>;

impl SpmError {
    pub fn network(msg: impl ToString) -> Self {
        SpmError::Network(msg.to_string())
    }

    pub fn compression(msg: impl ToString) -> Self {
        SpmError::Compression(msg.to_string())
    }

    pub fn package_not_found(msg: impl ToString) -> Self {
        SpmError::PackageNotFound(msg.to_string())
    }

    pub fn package_already_installed(msg: impl ToString) -> Self {
        SpmError::PackageAlreadyInstalled(msg.to_string())
    }

    pub fn dependency_failed(msg: impl ToString) -> Self {
        SpmError::DependencyFailed(msg.to_string())
    }

    pub fn resolution_failed(msg: impl ToString) -> Self {
        SpmError::DependencyFailed(msg.to_string())
    }

    pub fn invalid_format(msg: impl ToString) -> Self {
        SpmError::InvalidFormat(msg.to_string())
    }

    pub fn permission_denied(msg: impl ToString) -> Self {
        SpmError::PermissionDenied(msg.to_string())
    }

    pub fn sandbox(msg: impl ToString) -> Self {
        SpmError::Sandbox(msg.to_string())
    }

    pub fn config(msg: impl ToString) -> Self {
        SpmError::Config(msg.to_string())
    }

    pub fn command_failed(msg: impl ToString) -> Self {
        SpmError::CommandFailed(msg.to_string())
    }

    pub fn other(msg: impl ToString) -> Self {
        SpmError::Other(msg.to_string())
    }

    /// Produce a user-facing error message suitable for CLI output.
    pub fn format_user_error(&self) -> String {
        match self {
            SpmError::PackageNotFound(pkg) => {
                format!("Package not found: {}. Try `spm update` first.", pkg)
            }
            SpmError::PackageAlreadyInstalled(pkg) => {
                format!("Package '{}' is already installed. Use --replace to reinstall.", pkg)
            }
            SpmError::CommandFailed(cmd) => {
                format!("Command failed: {}. Is the required tool installed?", cmd)
            }
            SpmError::Network(url) => {
                format!("Network error: {}. Check your internet connection.", url)
            }
            SpmError::Config(msg) => {
                format!("Configuration error: {}", msg)
            }
            SpmError::PermissionDenied(msg) => {
                format!("Permission denied: {}. Try running with sudo.", msg)
            }
            SpmError::Sandbox(msg) => format!("Sandbox error: {}", msg),
            SpmError::InvalidFormat(msg) => format!("Invalid package format: {}", msg),
            SpmError::DependencyFailed(msg) => {
                // PubGrub's DefaultStringReporter output is already fairly readable;
                // prefix it clearly and drop any inner "Dependency resolution failed:" noise.
                let clean = msg.strip_prefix("Dependency resolution failed: ").unwrap_or(msg);
                format!("Cannot satisfy dependencies. {clean}")
            }
            other => format!("Error: {}", other),
        }
    }
}

impl From<ureq::Error> for SpmError {
    fn from(e: ureq::Error) -> Self {
        SpmError::Network(e.to_string())
    }
}

impl From<toml::ser::Error> for SpmError {
    fn from(e: toml::ser::Error) -> Self {
        SpmError::TomlSerialize(e.to_string())
    }
}

impl From<anyhow::Error> for SpmError {
    fn from(e: anyhow::Error) -> Self {
        SpmError::Other(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_io() {
        let e = SpmError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"));
        let msg = format!("{}", e);
        assert!(msg.contains("I/O error"));
        assert!(msg.contains("file not found"));
    }

    #[test]
    fn test_display_network() {
        let e = SpmError::network("connection refused");
        assert_eq!(format!("{}", e), "Network error: connection refused");
    }

    #[test]
    fn test_display_package_not_found() {
        let e = SpmError::package_not_found("nginx");
        assert_eq!(format!("{}", e), "Package not found: nginx");
    }

    #[test]
    fn test_display_package_already_installed() {
        let e = SpmError::package_already_installed("nginx");
        assert_eq!(format!("{}", e), "Package already installed: nginx");
    }

    #[test]
    fn test_display_invalid_format() {
        let e = SpmError::invalid_format("bad magic");
        assert_eq!(format!("{}", e), "Invalid package format: bad magic");
    }

    #[test]
    fn test_display_permission_denied() {
        let e = SpmError::permission_denied("not root");
        assert_eq!(format!("{}", e), "Permission denied: not root");
    }

    #[test]
    fn test_display_config() {
        let e = SpmError::config("unknown key");
        assert_eq!(format!("{}", e), "Configuration error: unknown key");
    }

    #[test]
    fn test_display_command_failed() {
        let e = SpmError::command_failed("apt-get not found");
        assert_eq!(format!("{}", e), "Command failed: apt-get not found");
    }

    #[test]
    fn test_display_sandbox() {
        let e = SpmError::sandbox("bwrap not installed");
        assert_eq!(format!("{}", e), "Sandbox error: bwrap not installed");
    }

    #[test]
    fn test_display_dependency_failed() {
        let e = SpmError::dependency_failed("cycle detected");
        assert_eq!(format!("{}", e), "Dependency resolution failed: cycle detected");
    }

    #[test]
    fn test_display_compression() {
        let e = SpmError::compression("zstd error");
        assert_eq!(format!("{}", e), "Compression error: zstd error");
    }

    #[test]
    fn test_display_other() {
        let e = SpmError::other("something went wrong");
        assert_eq!(format!("{}", e), "something went wrong");
    }

    #[test]
    fn test_from_anyhow_error() {
        let anyhow_err = anyhow::anyhow!("something bad");
        let spm_err: SpmError = anyhow_err.into();
        assert!(format!("{}", spm_err) == "something bad");
    }

    #[test]
    fn test_format_user_error_package_not_found() {
        let e = SpmError::package_not_found("nginx");
        assert_eq!(e.format_user_error(), "Package not found: nginx. Try `spm update` first.");
    }

    #[test]
    fn test_format_user_error_already_installed() {
        let e = SpmError::package_already_installed("nginx");
        assert_eq!(e.format_user_error(), "Package 'nginx' is already installed. Use --replace to reinstall.");
    }

    #[test]
    fn test_format_user_error_command_failed() {
        let e = SpmError::command_failed("apt-get");
        assert_eq!(e.format_user_error(), "Command failed: apt-get. Is the required tool installed?");
    }

    #[test]
    fn test_format_user_error_network() {
        let e = SpmError::network("timeout");
        assert_eq!(e.format_user_error(), "Network error: timeout. Check your internet connection.");
    }

    #[test]
    fn test_format_user_error_config() {
        let e = SpmError::config("unknown key");
        assert_eq!(e.format_user_error(), "Configuration error: unknown key");
    }

    #[test]
    fn test_format_user_error_permission_denied() {
        let e = SpmError::permission_denied("not root");
        assert_eq!(e.format_user_error(), "Permission denied: not root. Try running with sudo.");
    }

    #[test]
    fn test_format_user_error_sandbox() {
        let e = SpmError::sandbox("bwrap missing");
        assert_eq!(e.format_user_error(), "Sandbox error: bwrap missing");
    }

    #[test]
    fn test_format_user_error_invalid_format() {
        let e = SpmError::invalid_format("bad magic");
        assert_eq!(e.format_user_error(), "Invalid package format: bad magic");
    }

    #[test]
    fn test_format_user_error_other() {
        let e = SpmError::other("something bad");
        assert_eq!(e.format_user_error(), "Error: something bad");
    }

    #[test]
    fn test_format_user_error_dependency_failed() {
        let e = SpmError::dependency_failed("No solution for nginx: conflicting requirements");
        assert_eq!(e.format_user_error(), "Cannot satisfy dependencies. No solution for nginx: conflicting requirements");
    }

    #[test]
    fn test_format_user_error_dependency_failed_strips_duplicate_prefix() {
        let e = SpmError::resolution_failed("Dependency resolution failed: circular deps");
        assert_eq!(e.format_user_error(), "Cannot satisfy dependencies. circular deps");
    }

    #[test]
    fn test_format_user_error_io_fallback() {
        let e = SpmError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "no file"));
        assert!(e.format_user_error().starts_with("Error:"));
    }

    #[test]
    fn test_constructors() {
        let e = SpmError::network("timeout");
        assert!(matches!(e, SpmError::Network(_)));

        let e = SpmError::config("bad key");
        assert!(matches!(e, SpmError::Config(_)));

        let e = SpmError::sandbox("no bwrap");
        assert!(matches!(e, SpmError::Sandbox(_)));
    }
}
