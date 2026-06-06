use std::path::Path;

use chrono::Utc;

use crate::config::paths;
use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::types::*;

pub fn init_system(root: Option<&str>, from_system: bool, fix_backend: bool) -> SpmResult<()> {
    let target = root.map(Path::new).unwrap_or_else(|| Path::new("/"));

    crate::output::section("🚀 Initializing spm system");

    if !target.exists() {
        return Err(SpmError::other(format!(
            "Target directory does not exist: {}",
            target.display()
        )));
    }

    // Phase 1: Create directory structure
    let dirs = if root.is_some() {
        let base = target.join("var").join("lib").join("spm");
        let cache = target.join("var").join("cache").join("spm");
        let etc = target.join("etc").join("spm");
        vec![
            base.join("packages"),
            base.join("sandboxes"),
            base.join("scripts"),
            base.join("store").join("backend"),
            cache.join("archives"),
            cache.join("repos"),
            etc.join("repos.d"),
            etc.join("trusted-keys"),
        ]
    } else {
        vec![
            paths::packages_dir(),
            paths::sandboxes_dir(),
            paths::scripts_dir(),
            paths::store_backend_dir(),
            paths::archives_dir(),
            paths::repos_cache_dir(),
            paths::repos_config_dir(),
            paths::trusted_keys_dir(),
        ]
    };

    for d in &dirs {
        std::fs::create_dir_all(d)
            .map_err(|e| SpmError::other(format!("Failed to create {}: {}", d.display(), e)))?;
    }

    crate::output::step_info(format!("Created {} directories", dirs.len()));

    // Phase 2: Initialize database
    if root.is_none() {
        let conn = db::open_db()?;
        let _ = conn;
        crate::output::step_info("Database initialized");
    } else {
        let db_path = target.join("var").join("lib").join("spm").join("metadata.db");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::output::step_info(format!("Database path ready: {}", db_path.display()));
    }

    // Phase 3: Fix/install backends (copy bundled → store)
    if fix_backend || from_system || root.is_some() {
        install_backends()?;
    }

    // Phase 4: Optionally import all system packages
    if from_system {
        crate::output::step_info("Importing system packages...");
        let has_dpkg = std::process::Command::new("dpkg")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()
            .is_some_and(|s| s.success());

        let has_rpm = std::process::Command::new("rpm")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()
            .is_some_and(|s| s.success());

        let (system_pkgs, pkg_format): (Vec<(String, String)>, PackageFormat) = if has_dpkg {
            (query_dpkg_packages(), PackageFormat::Deb)
        } else if has_rpm {
            (query_rpm_packages(), PackageFormat::Rpm)
        } else {
            (Vec::new(), PackageFormat::Sam)
        };

        if system_pkgs.is_empty() {
            crate::output::step_warn("No system packages found to import");
        } else {
            let conn = db::open_db()?;
            let now = Utc::now().to_rfc3339();
            let mut imported = 0;
            for (name, version) in &system_pkgs {
                let pkg = InstalledPackage {
                    name: name.clone(),
                    version: version.clone(),
                    format: pkg_format.clone(),
                    install_type: InstallType::Native,
                    manifest: None,
                    install_date: now.clone(),
                    source_repo: Some("foreign:system".to_string()),
                    store_hash: None,
                    origin: InstallOrigin::Foreign,
                };
                db::add_installed_package(&conn, &pkg)?;
                imported += 1;
            }
            crate::output::step_info(format!("Imported {} system packages", imported));
        }
    }

    crate::output::result_message("spm initialized successfully");
    Ok(())
}

/// Copy bundled backends to the store directory.
/// If no bundled backends exist, attempt to copy them from the system (for
/// initial bootstrap where SPM was installed via the OS package manager).
fn install_backends() -> SpmResult<()> {
    // First try: copy from bundled /usr/libexec/spm/backend/
    match crate::backend::copy_bundled_to_store() {
        Ok(n) if n > 0 => {
            crate::output::step_info(format!("Installed {} backends from bundled", n));
            return Ok(());
        }
        Ok(_) => {
            crate::output::step_warn("No bundled backends found");
        }
        Err(e) => {
            tracing::debug!("Bundled backend copy failed: {e}");
        }
    }

    // Second try: copy from system (for transitional bootstrap)
    let backends = ["apt-get", "apt-cache", "dpkg-deb", "dpkg", "dnf", "rpm", "rpm2cpio", "cpio"];
    let mut copied = 0;

    for name in &backends {
        let dst_dir = paths::store_backend_dir().join(name).join("bin");
        std::fs::create_dir_all(&dst_dir).ok();

        let dst = dst_dir.join(name);
        if dst.exists() {
            copied += 1;
            continue;
        }

        // Check system PATH (only during transitional bootstrap — will be removed)
        if let Some(path) = find_on_path(name) {
            match std::fs::copy(&path, &dst) {
                Ok(_) => {
                    #[allow(unused_imports)]
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = std::fs::metadata(&dst) {
                        let mut perms = meta.permissions();
                        perms.set_mode(0o755);
                        std::fs::set_permissions(&dst, perms).ok();
                    }
                    copied += 1;
                }
                Err(e) => {
                    tracing::debug!("Failed to copy '{}' from system: {e}", name);
                }
            }
        }
    }

    if copied > 0 {
        crate::output::step_info(format!("Copied {} backends from system", copied));
    } else {
        crate::output::step_warn(
            "No backends could be installed. SPM will have limited functionality.\n\
             Install spm-backends package or ensure /usr/libexec/spm/backend/ exists."
        );
    }

    Ok(())
}

/// Search PATH and common system directories for a binary.
/// Used only during transitional bootstrap to populate the backend store.
fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    // First check PATH
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Try common locations directly
    for dir in &["/usr/bin", "/usr/sbin", "/bin", "/usr/lib/cpio"] {
        let candidate = std::path::PathBuf::from(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn query_dpkg_packages() -> Vec<(String, String)> {
    let output = std::process::Command::new("dpkg")
        .args(["--get-selections"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[1] == "install" {
            let name = parts[0].to_string();
            let ver_output = std::process::Command::new("dpkg")
                .args(["-s", &name])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output();
            if let Ok(o) = ver_output {
                let info = String::from_utf8_lossy(&o.stdout);
                if let Some(ver_line) = info.lines().find(|l| l.starts_with("Version: ")) {
                    let version = ver_line.trim_start_matches("Version: ").to_string();
                    packages.push((name, version));
                }
            }
        }
    }
    packages
}

fn query_rpm_packages() -> Vec<(String, String)> {
    let output = std::process::Command::new("rpm")
        .args(["-qa", "--queryformat", "%{NAME} %{VERSION}-%{RELEASE}\n"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut packages = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if parts.len() == 2 {
            packages.push((parts[0].to_string(), parts[1].to_string()));
        }
    }
    packages
}
