use std::collections::{HashMap, HashSet};
use std::io::Read;

use crate::config::repos;
use crate::db;
use crate::error::SpmResult;
use crate::package::transaction::TransactionEngine;
use crate::types::*;

pub fn upgrade_package(name: Option<&str>, strategy: VersionStrategy, preferred_format: Option<RepoSource>) -> SpmResult<()> {
    crate::output::section("📤 Upgrading packages");
    db::with_write_lock(|conn| {
        let dep_hashes_for = |pkg_name: &str, fmt: &PackageFormat, preferred: Option<RepoSource>| -> Vec<String> {
            let pid = PackageId::new(pkg_name, fmt.clone());
            if let Ok(resolved) = crate::package::resolver::resolve_dependencies_preferred(&pid, strategy, preferred) {
                resolved.topological_order.iter()
                    .filter(|d| d.name != pkg_name)
                    .filter_map(|d| db::get_store_hash(conn, &d.name, &d.format).ok().flatten())
                    .collect()
            } else {
                Vec::new()
            }
        };

        if let Some(pkg_name) = name {
            // Single package upgrade
            if let Some(pkg) = db::get_installed_package(conn, pkg_name)? {
                let repos_list = repos::load_repos()?;
                crate::output::step_info(format!("Checking {}...", pkg_name));
                match check_upgrade(&pkg, &repos_list)? {
                    UpgradeResult::UpgradeTo(avail) => {
                        crate::output::step_info(format!("  {}: {} → {}", pkg_name, pkg.version, avail));
                        let engine = TransactionEngine::new(conn);
                        engine.upgrade_package(pkg_name, &dep_hashes_for(pkg_name, &pkg.format, preferred_format.clone()))?;
                        crate::output::result_message(format!("Upgraded {}", pkg_name));
                    }
                    UpgradeResult::UpToDate => {
                        crate::output::step_info(format!("{} is already up to date ({}).", pkg_name, pkg.version));
                    }
                    UpgradeResult::Obsoleted => {
                        crate::output::step_warn(format!("{} is no longer available in repositories.", pkg_name));
                        crate::output::step_info("Use `spm remove` to uninstall it.");
                    }
                }
            } else {
                crate::output::step_warn(format!("Package '{}' is not installed.", pkg_name));
            }
        } else {
            // Full system upgrade (dist-upgrade)
            let installed = db::list_installed_packages(conn)?;
            let repos_list = repos::load_repos()?;

            // Phase 1: Check all packages and find upgradable ones
            let mut upgradable: Vec<(InstalledPackage, String)> = Vec::new();
            let mut up_to_date = 0;
            let mut obsoleted: Vec<String> = Vec::new();

            for pkg in &installed {
                match check_upgrade(pkg, &repos_list)? {
                    UpgradeResult::UpgradeTo(avail) => {
                        crate::output::step_info(format!("  {}: {} → {}", pkg.name, pkg.version, avail));
                        upgradable.push((pkg.clone(), avail));
                    }
                    UpgradeResult::UpToDate => {
                        up_to_date += 1;
                    }
                    UpgradeResult::Obsoleted => {
                        obsoleted.push(pkg.name.clone());
                    }
                }
            }

            // Phase 2: Show summary
            if upgradable.is_empty() && obsoleted.is_empty() {
                crate::output::result_message(format!("All {} packages up to date.", installed.len()));
                return Ok(());
            }

            crate::output::step_info(format!(
                "Found {} upgradable package(s), {} obsoleted, {} up to date",
                upgradable.len(), obsoleted.len(), up_to_date
            ));

            // Phase 3: Sort by dependency order (leaves first = dependencies before dependents)
            // This is approximate: we use topological_order from resolver
            let mut upgraded = 0;
            for (pkg, _avail) in &upgradable {
                let engine = TransactionEngine::new(conn);
                match engine.upgrade_package(&pkg.name, &dep_hashes_for(&pkg.name, &pkg.format, preferred_format.clone())) {
                    Ok(()) => {
                        upgraded += 1;
                        crate::output::result_message(format!("Upgraded {}", pkg.name));
                    }
                    Err(e) => {
                        crate::output::step_warn(format!("Failed to upgrade {}: {}", pkg.name, e));
                    }
                }
            }

            // Phase 4: Handle obsoleted packages
            if !obsoleted.is_empty() {
                crate::output::step_warn(format!(
                    "{} package(s) obsoleted (no longer in repos): {}",
                    obsoleted.len(), obsoleted.join(", ")
                ));
                crate::output::step_info("Use `spm remove <pkg>` to uninstall obsolete packages.");
            }

            crate::output::result_message(format!("Upgraded {} of {} packages", upgraded, upgradable.len()));
        }
        Ok(())
    })
}

enum UpgradeResult {
    UpgradeTo(String),
    UpToDate,
    Obsoleted,
}

fn format_to_source(format: &PackageFormat) -> RepoSource {
    match format {
        PackageFormat::Deb => RepoSource::Deb,
        PackageFormat::Rpm => RepoSource::Rpm,
        PackageFormat::Sam => RepoSource::Native,
    }
}

fn should_upgrade(installed_ver: &str, available_ver: &str) -> bool {
    if installed_ver.is_empty() {
        return true;
    }
    crate::types::Version::compare(available_ver, installed_ver).is_gt()
}

fn check_upgrade(pkg: &InstalledPackage, repos: &[(String, RepoConfig)]) -> SpmResult<UpgradeResult> {
    let installed_ver = &pkg.version;
    if installed_ver.is_empty() {
        return Ok(UpgradeResult::UpgradeTo("latest".into()));
    }

    let matching_source = format_to_source(&pkg.format);
    let mut found_in_any_repo = false;

    for (_rn, rc) in repos {
        if rc.source != matching_source {
            continue;
        }
        let available_ver: Option<String> = match rc.source {
            RepoSource::Deb => {
                // Read from cached Packages files (from spm update)
                let deb_cache = crate::config::paths::repos_cache_dir().join("deb").join(_rn);
                if !deb_cache.exists() {
                    None
                } else if let Ok(entries) = std::fs::read_dir(&deb_cache) {
                    let mut apt_version: Option<String> = None;
                    for entry in entries.flatten() {
                        if apt_version.is_some() {
                            break;
                        }
                        let p = entry.path();
                        if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
                            if !fname.starts_with("Packages-") {
                                continue;
                            }
                            if let Ok(text) = std::fs::read_to_string(&p) {
                                let mut in_pkg = false;
                                let mut version: Option<String> = None;
                                for line in text.lines() {
                                    if line.is_empty() {
                                        if in_pkg {
                                            apt_version = version.take();
                                            break;
                                        }
                                        in_pkg = false;
                                        continue;
                                    }
                                    if let Some(val) = line.strip_prefix("Package: ") {
                                        in_pkg = val.trim().eq_ignore_ascii_case(&pkg.name);
                                    }
                                    if in_pkg {
                                        if let Some(val) = line.strip_prefix("Version: ") {
                                            version = Some(val.trim().to_string());
                                        }
                                    }
                                }
                                if in_pkg {
                                    apt_version = version.take();
                                }
                            }
                        }
                    }
                    apt_version
                } else {
                    None
                }
            }
            RepoSource::Rpm => {
                // Read from SONAME index
                if let Ok(index) = crate::index::SonameIndex::load() {
                    if let Some(providers) = index.get_providers(&pkg.name) {
                        providers.iter()
                            .filter(|p| p.source == RepoSource::Rpm)
                            .map(|p| p.version.clone())
                            .next()
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            RepoSource::Native => {
                let url = match rc.url.as_deref() {
                    Some(u) => u,
                    None => continue,
                };
                let index_url = format!("{}/repo-index.json", url.trim_end_matches('/'));
                let response = match ureq::get(&index_url).call() {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let mut body = Vec::new();
                if response.into_body().into_reader().read_to_end(&mut body).is_err() {
                    continue;
                }
                let index: serde_json::Value = match serde_json::from_slice(&body) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match index["packages"].as_array() {
                    Some(arr) => {
                        arr.iter().find_map(|p| {
                            if p["name"].as_str()? == pkg.name {
                                p["version"].as_str().map(|v| v.to_string())
                            } else {
                                None
                            }
                        })
                    }
                    None => continue,
                }
            }
        };

        found_in_any_repo = true;

        if let Some(avail) = available_ver {
            if should_upgrade(installed_ver, &avail) {
                return Ok(UpgradeResult::UpgradeTo(avail));
            }
            return Ok(UpgradeResult::UpToDate);
        }
    }

    if found_in_any_repo {
        Ok(UpgradeResult::UpToDate)
    } else {
        Ok(UpgradeResult::Obsoleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_to_source_deb() {
        assert_eq!(format_to_source(&PackageFormat::Deb), RepoSource::Deb);
    }

    #[test]
    fn test_format_to_source_rpm() {
        assert_eq!(format_to_source(&PackageFormat::Rpm), RepoSource::Rpm);
    }

    #[test]
    fn test_format_to_source_sam() {
        assert_eq!(format_to_source(&PackageFormat::Sam), RepoSource::Native);
    }

    #[test]
    fn test_should_upgrade_empty_installed() {
        assert!(should_upgrade("", "2.0"));
    }

    #[test]
    fn test_should_upgrade_newer_available() {
        assert!(should_upgrade("1.0", "2.0"));
    }

    #[test]
    fn test_should_upgrade_same_version() {
        assert!(!should_upgrade("1.0", "1.0"));
    }

    #[test]
    fn test_should_upgrade_older_available() {
        assert!(!should_upgrade("2.0", "1.0"));
    }

    #[test]
    fn test_should_upgrade_with_epoch() {
        assert!(should_upgrade("1:1.0", "2:1.0"));
    }

    #[test]
    fn test_should_upgrade_empty_both() {
        assert!(should_upgrade("", ""));
    }
}

/// Perform a full distribution upgrade: upgrade all packages, auto-remove
/// obsoleted packages, and finally cleanup orphaned packages.
pub fn dist_upgrade_packages(yes: bool) -> SpmResult<()> {
    crate::output::section("📤 Distribution upgrade");
    db::with_write_lock(|conn| {
        let installed = db::list_installed_packages(conn)?;
        let repos_list = repos::load_repos()?;

        fn dep_hashes_for(conn: &rusqlite::Connection, pkg_name: &str, fmt: &PackageFormat) -> Vec<String> {
            let pid = PackageId::new(pkg_name, fmt.clone());
            if let Ok(resolved) = crate::package::resolver::resolve_dependencies_preferred(&pid, Default::default(), None) {
                resolved.topological_order.iter()
                    .filter(|d| d.name != pkg_name)
                    .filter_map(|d| db::get_store_hash(conn, &d.name, &d.format).ok().flatten())
                    .collect()
            } else {
                Vec::new()
            }
        }

        // Phase 1: Check all packages
        let mut upgradable: Vec<(InstalledPackage, String)> = Vec::new();
        let mut obsoleted: Vec<String> = Vec::new();
        for pkg in &installed {
            match check_upgrade(pkg, &repos_list)? {
                UpgradeResult::UpgradeTo(avail) => {
                    upgradable.push((pkg.clone(), avail));
                }
                UpgradeResult::UpToDate => {}
                UpgradeResult::Obsoleted => { obsoleted.push(pkg.name.clone()); }
            }
        }

        // Phase 2: Find orphans (packages no other package depends on)
        let all_names: HashSet<String> = installed.iter().map(|p| p.name.clone()).collect();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for pkg in &installed {
            if let Some(ref mj) = pkg.manifest {
                if let Ok(m) = serde_json::from_str::<Manifest>(mj) {
                    for dep in &m.dependencies {
                        if all_names.contains(&dep.name) {
                            dependents.entry(dep.name.clone())
                                .or_default()
                                .push(pkg.name.clone());
                        }
                    }
                }
            }
        }
        let orphans: Vec<String> = installed.iter()
            .filter(|p| {
                dependents.get(&p.name).map(|v| v.is_empty()).unwrap_or(true) && p.name != "spm"
            })
            .map(|p| p.name.clone())
            .filter(|n| !obsoleted.contains(n) && !upgradable.iter().any(|(p, _)| p.name == *n))
            .collect();

        if upgradable.is_empty() && obsoleted.is_empty() && orphans.is_empty() {
            crate::output::result_message(format!("All {} packages up to date.", installed.len()));
            return Ok(());
        }

        // Phase 3: Summary
        crate::output::step_info(format!(
            "{} upgradable, {} obsoleted (will be removed), {} orphans (will be removed)",
            upgradable.len(), obsoleted.len(), orphans.len(),
        ));

        if !yes {
            use std::io::Write;
            eprint!("  {} Proceed with dist-upgrade? [y/N]: ",
                crate::output::cyan("?"),
            );
            let _ = std::io::stdout().flush();
            let mut buf = String::new();
            if std::io::stdin().read_line(&mut buf).is_ok() {
                let input = buf.trim().to_lowercase();
                if input != "y" && input != "yes" {
                    crate::output::step_info("Cancelled.");
                    return Ok(());
                }
            } else {
                crate::output::step_info("Cancelled.");
                return Ok(());
            }
        }

        // Phase 4: Upgrade all
        let mut upgraded = 0;
        for (pkg, _avail) in &upgradable {
            let engine = TransactionEngine::new(conn);
            match engine.upgrade_package(&pkg.name, &dep_hashes_for(conn, &pkg.name, &pkg.format)) {
                Ok(()) => {
                    upgraded += 1;
                    crate::output::result_message(format!("Upgraded {}", pkg.name));
                }
                Err(e) => {
                    crate::output::step_warn(format!("Failed to upgrade {}: {}", pkg.name, e));
                }
            }
        }

        // Phase 5: Remove obsoleted packages
        for name in &obsoleted {
            crate::output::step_info(format!("Removing obsolete '{}'...", name));
            let _ = crate::package::install::remove_package(name);
        }

        // Phase 6: Remove orphaned packages
        for name in &orphans {
            crate::output::step_info(format!("Removing orphan '{}'...", name));
            let _ = crate::package::install::remove_package(name);
        }

        crate::output::result_message(format!(
            "Dist-upgrade complete: {} upgraded, {} obsoleted removed, {} orphans removed",
            upgraded, obsoleted.len(), orphans.len(),
        ));
        Ok(())
    })
}
