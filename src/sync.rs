use std::collections::HashSet;
use std::path::Path;

use chrono::Utc;

use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::types::*;

fn parse_deb822_status(path: &str) -> Vec<(String, String)> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut packages = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_ver: Option<String> = None;

    for line in content.lines() {
        if line.trim().is_empty() {
            if let (Some(name), Some(version)) = (current_name.take(), current_ver.take()) {
                packages.push((name, version));
            }
            continue;
        }
        if let Some(val) = line.strip_prefix("Package: ") {
            current_name = Some(val.trim().to_string());
        }
        if let Some(val) = line.strip_prefix("Version: ") {
            current_ver = Some(val.trim().to_string());
        }
    }
    if let (Some(name), Some(version)) = (current_name, current_ver) {
        packages.push((name, version));
    }

    packages
}

fn query_dpkg_files(name: &str) -> Vec<String> {
    let list_path = format!("/var/lib/dpkg/info/{}.list", name);
    match std::fs::read_to_string(&list_path) {
        Ok(content) => content
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn query_rpm_packages() -> Vec<(String, String)> {
    // Try reading RPM database via sqlite (modern Fedora).
    let sqlite_path = std::path::Path::new("/var/lib/rpm/rpmdb.sqlite");
    if sqlite_path.exists() {
        if let Ok(conn) = rusqlite::Connection::open(sqlite_path) {
            if let Ok(mut stmt) = conn.prepare("SELECT name, version || '-' || release FROM packages") {
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                    ))
                });
                if let Ok(rows) = rows {
                    let pkgs: Vec<_> = rows.filter_map(|r| r.ok()).collect();
                    if !pkgs.is_empty() {
                        return pkgs;
                    }
                }
            }
        }
    }
    // For BerkeleyDB /var/lib/rpm/Packages we cannot parse without librpm
    Vec::new()
}

fn query_rpm_files(name: &str) -> Vec<String> {
    // Try reading file list from rpm sqlite database
    let sqlite_path = std::path::Path::new("/var/lib/rpm/rpmdb.sqlite");
    if sqlite_path.exists() {
        if let Ok(conn) = rusqlite::Connection::open(sqlite_path) {
            if let Ok(mut stmt) = conn.prepare(
                "SELECT fi.name FROM files fi \
                 JOIN packages p ON p.packageId = fi.packageId \
                 WHERE p.name = ?1"
            ) {
                if let Ok(rows) = stmt.query_map([name], |row| row.get::<_, String>(0)) {
                    let files: Vec<_> = rows.filter_map(|r| r.ok()).collect();
                    if !files.is_empty() {
                        return files;
                    }
                }
            }
        }
    }
    Vec::new()
}

pub fn sync_system(files: bool, prune: bool) -> SpmResult<()> {
    let has_dpkg = Path::new("/var/lib/dpkg/status").exists();
    let has_rpm = Path::new("/var/lib/rpm/Packages").exists()
        || Path::new("/var/lib/rpm/rpmdb.sqlite").exists();

    if !has_dpkg && !has_rpm {
        return Err(SpmError::other(
            "No supported system package manager found (dpkg/rpm). Nothing to sync."
        ));
    }

    let (system_pkgs, pkg_format): (Vec<(String, String)>, PackageFormat) = if has_dpkg {
        (parse_deb822_status("/var/lib/dpkg/status"), PackageFormat::Deb)
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
                    let file_list: Vec<String> = if has_dpkg {
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

        if prune {
            let mut pruned = 0;
            for name in &spm_names {
                if !system_names.contains(name) {
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

        let (spm_count, foreign_count) = db::count_packages_by_origin(conn)?;
        crate::output::result_message(format!(
            "Sync complete: {} spm-managed + {} foreign = {} total",
            spm_count, foreign_count, spm_count + foreign_count
        ));

        Ok(())
    })
}
