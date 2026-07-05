use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use chrono::Utc;

use crate::config::paths;
use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::types::*;

pub fn init_system(root: Option<&str>, from_system: bool, _fix_backend: bool) -> SpmResult<()> {
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

    // Phase 3: Optionally import all system packages
    if from_system {
        crate::output::step_info("Importing system packages...");
        let has_dpkg = Path::new("/var/lib/dpkg/status").exists();
        let has_rpm = Path::new("/var/lib/rpm/Packages").exists()
            || Path::new("/var/lib/rpm/rpmdb.sqlite").exists();

        let (system_pkgs, pkg_format): (Vec<(String, String)>, PackageFormat) = if has_rpm {
            (query_rpm_packages(), PackageFormat::Rpm)
        } else if has_dpkg {
            (query_dpkg_packages(), PackageFormat::Deb)
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

fn query_dpkg_packages() -> Vec<(String, String)> {
    // Read /var/lib/dpkg/status directly
    let path = std::path::Path::new("/var/lib/dpkg/status");
    if !path.exists() {
        return Vec::new();
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut packages = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_version: Option<String> = None;
    let mut status_install = false;

    for line in content.lines() {
        if line.trim().is_empty() {
            if let (Some(name), Some(version)) = (current_name.take(), current_version.take()) {
                if status_install {
                    packages.push((name, version));
                }
            }
            current_version = None;
            status_install = false;
            continue;
        }
        if let Some(val) = line.strip_prefix("Package: ") {
            current_name = Some(val.trim().to_string());
        }
        if let Some(val) = line.strip_prefix("Version: ") {
            current_version = Some(val.trim().to_string());
        }
        if line.starts_with("Status: ") && line.contains("installed") {
            status_install = true;
        }
    }
    if let (Some(name), Some(version)) = (current_name, current_version) {
        if status_install {
            packages.push((name, version));
        }
    }
    packages
}

fn query_rpm_packages() -> Vec<(String, String)> {
    // Try reading RPM sqlite database (modern Fedora)
    let sqlite_path = std::path::Path::new("/var/lib/rpm/rpmdb.sqlite");
    if sqlite_path.exists() {
        if let Ok(conn) = rusqlite::Connection::open(sqlite_path) {
            if let Ok(mut stmt) = conn.prepare("SELECT name, version || '-' || release FROM packages") {
                if let Ok(rows) = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                    ))
                }) {
                    let pkgs: Vec<_> = rows.filter_map(|r| r.ok()).collect();
                    if !pkgs.is_empty() {
                        return pkgs;
                    }
                }
            }
        }
    }
    Vec::new()
}

/// Install the spmd systemd service unit for daemon-based operations.
/// This requires root (to write to /etc/systemd/system/) and a running
/// systemd.  Called from `spm init --install-daemon`.
pub fn install_daemon_service() -> SpmResult<()> {
    let service_path = Path::new("/etc/systemd/system/spmd.service");

    // Only write if not already present (don't overwrite user modifications)
    // or if the unit is outdated (missing ProtectSystem=full, or has old ProtectHome=yes)
    let needs_write = if service_path.exists() {
        let existing = fs::read_to_string(service_path).unwrap_or_default();
        !existing.contains("ProtectSystem=full") || existing.contains("ProtectHome=yes")
    } else {
        true
    };

    if needs_write {
        let unit = r#"[Unit]
Description=SPM Daemon — privileged package operations
Documentation=man:spmd(8)
After=network.target local-fs.target

[Service]
Type=simple
ExecStart=/usr/local/bin/spmd
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info
ProtectSystem=full
ReadWritePaths=/var/lib/spm /var/cache/spm /etc/spm

[Install]
WantedBy=multi-user.target
"#;

        fs::write(service_path, unit)
            .map_err(|e| SpmError::other(format!("Cannot write {service_path:?}: {e}")))?;

        // 0644 — world-readable, root-writable (standard for systemd units)
        fs::set_permissions(service_path, fs::Permissions::from_mode(0o644))
            .map_err(|e| SpmError::other(format!("Cannot set permissions on {service_path:?}: {e}")))?;

        crate::output::step_success(format!("Wrote systemd unit: {service_path:?}"));
    } else {
        crate::output::step_info("spmd.service already installed");
    }

    // Reload systemd, enable, start
    let steps: &[(&str, &[&str], &str)] = &[
        ("systemctl", &["daemon-reload"], "systemd daemon-reload"),
        ("systemctl", &["enable", "spmd"], "enable spmd"),
        ("systemctl", &["start", "spmd"], "start spmd"),
    ];
    for (cmd, args, label) in steps {
        let output = Command::new(cmd)
            .args(*args)
            .output()
            .map_err(|e| SpmError::command_failed(format!("{label} failed: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            crate::output::step_warn(format!("{label} failed: {stderr}"));
        } else {
            crate::output::step_success(format!("{label}"));
        }
    }

    crate::output::result_message("spmd daemon service installed and started");
    Ok(())
}
