use std::collections::HashSet;

use chrono::Utc;

use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::types::*;

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
            // Query version
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

fn query_dpkg_files(name: &str) -> Vec<String> {
    let output = std::process::Command::new("dpkg")
        .args(["-L", name])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        }
        _ => Vec::new(),
    }
}

fn query_rpm_files(name: &str) -> Vec<String> {
    let output = std::process::Command::new("rpm")
        .args(["-ql", name])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        }
        _ => Vec::new(),
    }
}

pub fn sync_system(files: bool, prune: bool) -> SpmResult<()> {
    // Phase 1: Detect which package manager is available
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

    if !has_dpkg && !has_rpm {
        return Err(SpmError::other(
            "No supported system package manager found (dpkg/rpm). Nothing to sync."
        ));
    }

    let (system_pkgs, pkg_format): (Vec<(String, String)>, PackageFormat) = if has_dpkg {
        (query_dpkg_packages(), PackageFormat::Deb)
    } else {
        (query_rpm_packages(), PackageFormat::Rpm)
    };

    crate::output::section("🔄 Syncing spm database with system packages");
    crate::output::step_info(format!("Found {} system packages", system_pkgs.len()));

    db::with_write_lock(|conn| {
        let spm_names: HashSet<String> = db::get_all_installed_package_names(conn)?
            .into_iter()
            .collect();

        let system_names: HashSet<String> = system_pkgs.iter().map(|(n, _)| n.clone()).collect();

        // Phase 2: Import foreign packages (in system but not in spm DB)
        let mut imported = 0;
        let now = Utc::now().to_rfc3339();

        for (name, version) in &system_pkgs {
            if !spm_names.contains(name) {
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
                db::add_installed_package(conn, &pkg)?;

                if files {
                    let file_list = if has_dpkg {
                        query_dpkg_files(name)
                    } else {
                        query_rpm_files(name)
                    };
                    if !file_list.is_empty() {
                        let file_records: Vec<FileRecord> = file_list.into_iter().map(|fp| FileRecord {
                            id: None,
                            transaction_id: 0,
                            package: name.clone(),
                            format: pkg_format.clone(),
                            filepath: fp,
                            hash: String::new(),
                            action: FileAction::Created,
                        }).collect();
                        db::record_files(conn, &file_records)?;
                    }
                }

                imported += 1;
            }
        }

        crate::output::step_info(format!("Imported {} foreign packages", imported));

        // Phase 3: Prune stale spm entries (in spm DB but not on system)
        if prune {
            let mut pruned = 0;
            for name in &spm_names {
                if !system_names.contains(name) {
                    // Only prune foreign packages automatically
                    if let Some(pkg) = db::get_installed_package(conn, name)? {
                        if matches!(pkg.origin, InstallOrigin::Foreign) {
                            db::remove_installed_package(conn, name)?;
                            pruned += 1;
                        }
                    }
                }
            }
            crate::output::step_info(format!("Pruned {} stale foreign entries", pruned));
        }

        // Phase 4: Show summary
        let (spm_count, foreign_count) = db::count_packages_by_origin(conn)?;
        crate::output::result_message(format!(
            "Sync complete: {} spm-managed + {} foreign = {} total",
            spm_count, foreign_count, spm_count + foreign_count
        ));

        Ok(())
    })
}
