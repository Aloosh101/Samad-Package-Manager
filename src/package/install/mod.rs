#[cfg(test)]
mod tests {
    #[test]
    fn test_source_to_format() {
        assert_eq!(super::source_to_format(&super::RepoSource::Apt), super::PackageFormat::Deb);
        assert_eq!(super::source_to_format(&super::RepoSource::Dnf), super::PackageFormat::Rpm);
        assert_eq!(super::source_to_format(&super::RepoSource::Native), super::PackageFormat::Sam);
    }
}

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::IsTerminal;
use std::process::Command;

use crate::config::{paths, repos};
use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::package::{resolver, transaction::TransactionEngine};
use crate::types::*;
use crate::verify;

mod local;
mod sandbox;
mod remove;
mod user;
mod upgrade;

pub use remove::{purge_package, remove_package};
pub use upgrade::{dist_upgrade_packages, upgrade_package};
pub use self::local::install_local_package;
pub use user::{install_for_user, remove_for_user};

fn ensure_dirs() -> SpmResult<()> {
    let dirs = [
        paths::packages_dir(),
        paths::sandboxes_dir(),
        paths::archives_dir(),
        paths::repos_cache_dir(),
        paths::repos_config_dir(),
    ];
    for d in &dirs {
        fs::create_dir_all(d)?;
    }
    Ok(())
}

pub fn install_package(name: &str, sandbox: Option<&str>, replace: bool, yes: bool, strategy: VersionStrategy) -> SpmResult<()> {
    install_package_smart(name, sandbox, replace, yes, false, strategy, None)
}

pub fn install_package_smart(
    name: &str,
    sandbox: Option<&str>,
    replace: bool,
    yes: bool,
    smart: bool,
    strategy: VersionStrategy,
    preferred_format: Option<RepoSource>,
) -> SpmResult<()> {
    crate::output::section(format!("📦 Installing {}", name));
    ensure_dirs()?;

    if let Some(mode) = sandbox {
        return db::with_write_lock(|conn| {
            if paths::sandbox_dir(name).exists() && !replace {
                return Err(SpmError::other(format!(
                    "Sandbox '{}' already exists. Use --replace to recreate.", name
                )));
            }
            sandbox::install_sandboxed(name, mode, conn, yes)
        });
    }

    let repos_list = repos::load_repos()?;
    let candidates: Vec<&(String, RepoConfig)> = repos_list
        .iter()
        .filter(|(rn, rc)| repo_has_package(name, rn, rc))
        .collect();

    if candidates.is_empty() {
        return Err(SpmError::package_not_found(format!(
            "Package '{name}' not found in any repository. Check the repository list with 'spm repo list'."
        )));
    }

    let mut last_err = None;
    for (repo_name, repo_config) in &candidates {
        let target_format = source_to_format(&repo_config.source);
        // Auto-enable smart mode for cross-distro installs
        let effective_smart = smart || crate::package::store::is_cross_distro(&target_format);
        match try_install_from_repo(name, target_format, repo_name, sandbox, replace, yes, effective_smart, strategy, preferred_format.clone()) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let is_fetch_err = matches!(&e, SpmError::Network(_) | SpmError::Io(_) | SpmError::Compression(_));
                if is_fetch_err && candidates.len() > 1 {
                    tracing::warn!("Failed to fetch '{}' from repo '{}': {e}. Trying next repo...", name, repo_name);
                    crate::output::step_warn(format!(
                        "Failed to fetch from repo '{}'. Trying next available repo...", repo_name
                    ));
                    last_err = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| SpmError::package_not_found(format!(
        "Package '{name}' not found in any repository."
    ))))
}

#[allow(clippy::too_many_arguments)]
fn try_install_from_repo(
    name: &str,
    target_format: PackageFormat,
    _repo_name: &str,
    sandbox: Option<&str>,
    replace: bool,
    yes: bool,
    smart: bool,
    strategy: VersionStrategy,
    preferred_format: Option<RepoSource>,
) -> SpmResult<()> {
    let pid = PackageId::new(name, target_format);
    let resolved = resolver::resolve_dependencies_preferred(&pid, strategy, preferred_format)?;

    if !resolved.cycles.is_empty() {
        crate::output::step_warn(format!("Dependency cycles detected: {:?}", resolved.cycles));
    }
    if !resolved.unresolved.is_empty() {
        let names: Vec<&str> = resolved.unresolved.iter().map(|p| p.name.as_str()).collect();
        crate::output::step_warn(format!("Some dependencies could not be resolved: {}", names.join(", ")));
    }

    // Handle metadata conflicts with sandbox suggestion (zypper-style)
    if !resolved.metadata_conflicts.is_empty() {
        for mc in &resolved.metadata_conflicts {
            crate::output::step_warn(format!(
                "{} conflicts with {}. This package may not work correctly.",
                mc.package, mc.conflicts_with,
            ));
        }
        if sandbox.is_none() && std::io::stdin().is_terminal() && !yes {
            use std::io::Write;
            eprint!("  {} Install in sandbox instead? {} ",
                crate::output::cyan("?"),
                crate::output::dim("(yes-f/sd/st, no-n)"),
            );
            let _ = std::io::stdout().flush();
            let mut buf = String::new();
            if std::io::stdin().read_line(&mut buf).is_ok() {
                match buf.trim().to_lowercase().as_str() {
                    "f" | "full" => return install_package(name, Some("full"), replace, yes, strategy),
                    "sd" | "standard" => return install_package(name, Some("standard"), replace, yes, strategy),
                    "st" | "strict" => return install_package(name, Some("strict"), replace, yes, strategy),
                    "y" | "yes" => return install_package(name, Some("standard"), replace, yes, strategy),
                    _ => {}
                }
            }
        }
    }

    let all_names: Vec<&str> = resolved.topological_order.iter().map(|p| p.name.as_str()).collect();
    if !verify::prompt_before_install(&[name], &all_names, yes)? {
        return Err(SpmError::other(format!("Installation of '{name}' cancelled.")));
    }

    // Phase 0: Build plan (read lock)
    let plan = db::with_read_lock(|conn| {
        let engine = TransactionEngine::new(conn);
        engine.plan_install(name, &resolved)
    })?;

    // Phase 1: Display + Approve
    let has_conflicts = !plan.to_remove.is_empty()
        || !plan.file_conflicts.is_empty()
        || {
            let (c, s, m) = &plan.classified;
            !c.is_empty() || !s.is_empty() || !m.is_empty()
        };

    TransactionEngine::display_plan_smart(&plan, smart);

    // Prompt for sandbox when file conflicts detected (interactive, exceptional)
    if has_conflicts && sandbox.is_none() && std::io::stdin().is_terminal() && !yes {
        use std::io::Write;
        eprint!("  {} File conflicts detected. Use sandbox? {} ",
            crate::output::cyan("?"),
            crate::output::dim("(full-f, standard-sd, strict-st, N)"),
        );
        let _ = std::io::stdout().flush();
        let mut buf = String::new();
        if std::io::stdin().read_line(&mut buf).is_ok() {
            match buf.trim().to_lowercase().as_str() {
                "f" | "full" => return install_package(name, Some("full"), replace, yes, strategy),
                "sd" | "standard" => return install_package(name, Some("standard"), replace, yes, strategy),
                "st" | "strict" => return install_package(name, Some("strict"), replace, yes, strategy),
                _ => {}
            }
        }
    }

    if !TransactionEngine::approve_plan(&plan, yes)? {
        return Err(SpmError::other(format!("Installation of '{name}' cancelled.")));
    }

    // Phase 2: Execute (write lock) — TransactionEngine handles all removes atomically
    db::with_write_lock(|conn| {
        if db::get_installed_package(conn, name)?.is_some() && !replace {
            return Err(SpmError::package_already_installed(name));
        }
        let engine = TransactionEngine::new(conn);
        engine.execute_smart(plan, replace, smart)
    })
}

pub fn install_package_from_repo(name: &str, source: RepoSource, replace: bool, yes: bool, smart: bool, strategy: VersionStrategy, preferred_format: Option<RepoSource>) -> SpmResult<()> {
    crate::output::section(format!("📦 Installing {} from {}", name, source));
    ensure_dirs()?;

    let repos_list = repos::load_repos()?;
    let (repo_name, repo_cfg) = repos_list.iter()
        .find(|(_, rc)| rc.source == source)
        .ok_or_else(|| SpmError::config(format!("No {} repository configured", source)))?;

    if !repo_has_package(name, repo_name, repo_cfg) {
        return Err(SpmError::package_not_found(format!(
            "Package '{name}' not found in {} repository", source,
        )));
    }

    let target_format = source_to_format(&source);
    // Auto-enable smart mode for cross-distro installs
    let smart = smart || crate::package::store::is_cross_distro(&target_format);

    let pid = PackageId::new(name, target_format);
    let resolved = resolver::resolve_dependencies_preferred(&pid, strategy, preferred_format)?;

    if !resolved.cycles.is_empty() {
        crate::output::step_warn(format!("Dependency cycles detected: {:?}", resolved.cycles));
    }
    if !resolved.unresolved.is_empty() {
        let names: Vec<&str> = resolved.unresolved.iter().map(|p| p.name.as_str()).collect();
        crate::output::step_warn(format!("Some dependencies could not be resolved: {}", names.join(", ")));
    }

    let all_names: Vec<&str> = resolved.topological_order.iter().map(|p| p.name.as_str()).collect();
    if !verify::prompt_before_install(&[name], &all_names, yes)? {
        return Err(SpmError::other(format!("Installation of '{name}' cancelled.")));
    }

    let plan = db::with_read_lock(|conn| {
        let engine = TransactionEngine::new(conn);
        engine.plan_install(name, &resolved)
    })?;

    TransactionEngine::display_plan_smart(&plan, smart);
    if !TransactionEngine::approve_plan(&plan, yes)? {
        return Err(SpmError::other(format!("Installation of '{name}' cancelled.")));
    }

    db::with_write_lock(|conn| {
        if db::get_installed_package(conn, name)?.is_some() && !replace {
            return Err(SpmError::package_already_installed(name));
        }
        let engine = TransactionEngine::new(conn);
        engine.execute_smart(plan, replace, smart)
    })
}



pub(crate) fn repo_has_package(name: &str, repo_name: &str, repo_config: &RepoConfig) -> bool {
    match repo_config.source {
        RepoSource::Apt => {
            let has_apt_cache = Command::new(crate::util::backend::resolve("apt-cache"))
                .args(["show", name])
                .stderr(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .status()
                .ok()
                .is_some_and(|s| s.success());
            if has_apt_cache {
                return true;
            }
            // Fall back to cached Packages.gz (HTTP-based apt)
            let apt_cache = crate::config::paths::repos_cache_dir().join("apt");
            if apt_cache.exists() {
                if let Ok(entries) = fs::read_dir(&apt_cache) {
                    for entry in entries.flatten() {
                        let dir = entry.path();
                        if !dir.is_dir() {
                            continue;
                        }
                        if let Ok(pkg_entries) = fs::read_dir(&dir) {
                            for pkg_entry in pkg_entries.flatten() {
                                let pkg_path = pkg_entry.path();
                                if let Some(name_str) = pkg_path.file_name().and_then(|s| s.to_str()) {
                                    if name_str.starts_with("Packages-") {
                                        if let Ok(text) = fs::read_to_string(&pkg_path) {
                                            for line in text.lines() {
                                                if let Some(pkg_name) = line.strip_prefix("Package: ") {
                                                    if pkg_name.eq_ignore_ascii_case(name) {
                                                        return true;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            false
        }
        RepoSource::Dnf => {
            Command::new(crate::util::backend::resolve("dnf"))
                .args(["repoquery", "--info", name])
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .is_some_and(|o| o.status.success() && !o.stdout.is_empty())
        }
        RepoSource::Native => {
            // Check the cached repo index for the package
            let cache_dir = crate::config::paths::repos_cache_dir().join("native").join(repo_name);
            let index_path = cache_dir.join("repo-index.json");
            if let Ok(content) = fs::read_to_string(&index_path) {
                if let Ok(index) = serde_json::from_str::<crate::types::RepoIndex>(&content) {
                    return index.packages.iter().any(|p| p.name == name);
                }
            }
            false
        }
    }
}

fn source_to_format(source: &RepoSource) -> PackageFormat {
    match source {
        RepoSource::Apt => PackageFormat::Deb,
        RepoSource::Dnf => PackageFormat::Rpm,
        RepoSource::Native => PackageFormat::Sam,
    }
}

pub fn autoremove_packages(yes: bool) -> SpmResult<()> {
    let conn = db::get_connection()?;
    let installed = db::list_installed_packages(&conn)?;

    if installed.is_empty() {
        crate::output::step_info("No packages installed.");
        return Ok(());
    }

    let all_names: HashSet<String> = installed.iter().map(|p| p.name.clone()).collect();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    for pkg in &installed {
        if let Some(ref manifest_json) = pkg.manifest {
            if let Ok(manifest) = serde_json::from_str::<Manifest>(manifest_json) {
                for dep in &manifest.dependencies {
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
            let deps = dependents.get(&p.name);
            deps.map(|v| v.is_empty()).unwrap_or(true) && p.name != "spm"
        })
        .map(|p| p.name.clone())
        .collect();

    if orphans.is_empty() {
        crate::output::step_info("No orphaned packages found.");
        return Ok(());
    }

    crate::output::section("🧹 Removing orphaned packages");
    for name in &orphans {
        println!("  {}", name);
    }

    if !yes {
        use std::io::Write;
        eprint!("  {} Remove these {} packages? [y/N]: ",
            crate::output::cyan("?"),
            orphans.len(),
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

    for name in &orphans {
        if let Err(e) = remove_package(name) {
            crate::output::step_warn(format!("Failed to remove '{}': {e}", name));
        }
    }

    crate::output::step_info(format!("Removed {} orphaned packages.", orphans.len()));
    Ok(())
}


