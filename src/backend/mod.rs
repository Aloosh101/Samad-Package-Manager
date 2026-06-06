use std::path::PathBuf;

use crate::config::paths;
use crate::error::{SpmError, SpmResult};

/// Known backends that SPM may require at runtime.
const BACKENDS: &[&str] = &["apt-get", "apt-cache", "dpkg-deb", "dpkg", "dnf", "rpm", "rpm2cpio", "cpio"];

/// Check whether every backend needed by the host distribution is available.
/// Returns a list of missing backend names.
pub fn check_missing() -> Vec<&'static str> {
    let host_source = crate::config::repos::detect_source();
    let required: &[&str] = match host_source {
        crate::types::RepoSource::Apt => &["apt-get", "apt-cache", "dpkg-deb", "dpkg"],
        crate::types::RepoSource::Dnf => &["dnf", "rpm", "rpm2cpio", "cpio"],
        crate::types::RepoSource::Native => &[],
    };

    let mut missing = Vec::new();
    for name in required {
        let path = resolve_backend_path(name);
        if !path.exists() || path.to_string_lossy().contains("spm-backend-missing") {
            missing.push(*name);
        }
    }
    missing
}

/// Print a one-time warning banner if any backends are missing.
/// Called at the start of every `spm` invocation.
pub fn show_warnings() {
    // Only check once per process
    use std::sync::OnceLock;
    static CHECKED: OnceLock<Vec<&'static str>> = OnceLock::new();

    let missing = CHECKED.get_or_init(check_missing);
    if missing.is_empty() {
        return;
    }

    eprintln!();
    eprintln!("  ⚠ SPM backend warning");
    for name in missing {
        eprintln!("     Backend '{}' is not available.", name);
    }
    eprintln!("     System package manager will not work until it is restored.");
    eprintln!("     Run: sudo spm init --fix-backend");
    eprintln!();
}

/// Copy all bundled backends from /usr/libexec/spm/backend/ into the store.
/// Returns the number of backends copied.
pub fn copy_bundled_to_store() -> SpmResult<usize> {
    let bundled_dir = PathBuf::from(crate::util::backend::bundled_dir());
    if !bundled_dir.is_dir() {
        return Err(SpmError::other(format!(
            "Bundled backend directory not found at {}",
            bundled_dir.display()
        )));
    }

    let mut count = 0;
    for name in BACKENDS {
        let src = bundled_dir.join(name);
        if !src.is_file() {
            continue;
        }

        let dst_dir = paths::store_backend_dir().join(name).join("bin");
        std::fs::create_dir_all(&dst_dir)
            .map_err(|e| SpmError::other(format!("Cannot create backend dir: {e}")))?;

        let dst = dst_dir.join(name);

        // Copy the binary
        std::fs::copy(&src, &dst)
            .map_err(|e| SpmError::other(format!("Cannot copy backend '{}': {e}", name)))?;

        // Make executable
        let mut perms = std::fs::metadata(&dst)
            .map_err(|e| SpmError::other(format!("Cannot read metadata: {e}")))?
            .permissions();
        #[allow(unused_imports)]
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        std::fs::set_permissions(&dst, perms)
            .map_err(|e| SpmError::other(format!("Cannot set permissions: {e}")))?;

        count += 1;
    }

    Ok(count)
}

/// Download and install a backend from a mirror URL into the store.
/// Used when bundled backends are missing (e.g. after RPM removal).
pub fn download_backend(name: &str, url: &str) -> SpmResult<()> {
    let dst_dir = paths::store_backend_dir().join(name).join("bin");
    std::fs::create_dir_all(&dst_dir)
        .map_err(|e| SpmError::other(format!("Cannot create backend dir: {e}")))?;

    let dst = dst_dir.join(name);

    // Download the backend binary
    let response = crate::config::repos::fetch_with_retry(url, 3)?;

    std::fs::write(&dst, &response)
        .map_err(|e| SpmError::other(format!("Cannot write backend binary: {e}")))?;

    // Make executable
    #[allow(unused_imports)]
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&dst)
        .map_err(|e| SpmError::other(format!("Cannot read metadata: {e}")))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&dst, perms)
        .map_err(|e| SpmError::other(format!("Cannot set permissions: {e}")))?;

    Ok(())
}

/// List backends currently registered in the store.
pub fn list_store_backends() -> SpmResult<Vec<String>> {
    let dir = paths::store_backend_dir();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut backends = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|e| SpmError::other(format!("Cannot read backend dir: {e}")))?
    {
        let entry = entry.map_err(|e| SpmError::other(format!("Dir entry error: {e}")))?;
        if entry.path().is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                backends.push(name.to_string());
            }
        }
    }
    Ok(backends)
}

fn resolve_backend_path(name: &str) -> PathBuf {
    // Same logic as util::backend::resolve but without the public dependency
    let store = paths::store_backend_dir()
        .join(name)
        .join("bin")
        .join(name);
    if store.exists() {
        return store;
    }
    let bundled = PathBuf::from(crate::util::backend::bundled_dir()).join(name);
    if bundled.exists() {
        return bundled;
    }
    PathBuf::from("/dev/null/spm-backend-missing")
}

/// Parse a dependency name + optional version constraint from deb822/RPM format.
pub fn parse_dep_entry(raw: &str) -> (String, String) {
    let raw = raw.trim();
    if let Some((name_part, constraint)) = raw.split_once('(') {
        let name = name_part.trim().to_string();
        let inner = constraint.trim_end_matches(')').trim();
        let (op, ver) = parse_op_ver(inner);
        return (name, format!("{} {}", op, ver));
    }
    for op in &[">=", "<=", ">>", "<<", "=", ">", "<"] {
        if let Some((name_part, ver)) = raw.split_once(op) {
            return (name_part.trim().to_string(), format!("{} {}", op, ver.trim()));
        }
    }
    let clean = raw.trim_matches(|c| c == '<' || c == '>' || c == '=' || c == ' ').to_string();
    (clean, String::new())
}

fn parse_op_ver(input: &str) -> (String, String) {
    let input = input.trim();
    for op in &[">=", "<=", ">>", "<<", "=", ">", "<"] {
        if let Some((_, ver)) = input.split_once(op) {
            return (op.to_string(), ver.trim().to_string());
        }
    }
    (String::new(), input.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_missing_no_panic() {
        // Should not crash even with no backends present
        let _ = check_missing();
    }

    #[test]
    fn test_parse_dep_entry_deb_format() {
        let (name, constraint) = parse_dep_entry("libssl (>= 1.1)");
        assert_eq!(name, "libssl");
        assert_eq!(constraint, ">= 1.1");
    }

    #[test]
    fn test_parse_dep_entry_rpm_format() {
        let (name, constraint) = parse_dep_entry("libssl >= 1.1");
        assert_eq!(name, "libssl");
        assert_eq!(constraint, ">= 1.1");
    }

    #[test]
    fn test_parse_dep_entry_no_constraint() {
        let (name, constraint) = parse_dep_entry("nginx");
        assert_eq!(name, "nginx");
        assert_eq!(constraint, "");
    }

    #[test]
    fn test_parse_dep_entry_equals() {
        let (name, v) = parse_dep_entry("foo (= 1.0)");
        assert_eq!(name, "foo");
        assert_eq!(v, "= 1.0");
    }

    #[test]
    fn test_parse_op_ver_typical() {
        assert_eq!(parse_op_ver(">= 1.2.3"), (">=".to_string(), "1.2.3".to_string()));
        assert_eq!(parse_op_ver("<= 5.0"), ("<=".to_string(), "5.0".to_string()));
        assert_eq!(parse_op_ver("= 2.0"), ("=".to_string(), "2.0".to_string()));
    }

    #[test]
    fn test_parse_op_ver_no_op() {
        assert_eq!(parse_op_ver("1.2.3"), ("".to_string(), "1.2.3".to_string()));
    }
}
