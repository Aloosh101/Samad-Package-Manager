use std::fs;
use std::path::Path;

use crate::config::repos;
use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::types::*;

use super::{repo_has_package, source_to_format};

pub fn install_for_user(name: &str, user_id: u32, user_home: &str) -> SpmResult<()> {
    let user_name = crate::util::user::resolve_user_name(user_id).unwrap_or_else(|| user_id.to_string());
    crate::output::section(format!("📦 Installing '{name}' for {user_name}"));
    let mut spinner = crate::output::Spinner::new(format!("Looking up '{name}' in repositories..."));

    let repos_list = repos::load_repos()?;

    let (_target_format, repo_name, repo_config) = repos_list
        .iter()
        .find(|(rn, rc)| repo_has_package(name, rn, rc))
        .map(|(rn, rc)| {
            let fmt = source_to_format(&rc.source);
            (fmt, rn.clone(), rc.clone())
        })
        .ok_or_else(|| SpmError::package_not_found(format!(
            "Package '{name}' not found in any repository"
        )))?;

    spinner.message(&format!("Found in repository '{repo_name}'"));

    let tmp_dir_obj = tempfile::tempdir()
        .map_err(|e| SpmError::other(format!("Failed to create temp directory: {e}")))?;
    let tmp_dir = tmp_dir_obj.path().to_string_lossy().to_string();
    let raw_dir = format!("{tmp_dir}/raw");
    fs::create_dir_all(&raw_dir)?;

    spinner.message("Downloading and extracting...");
    let result = match repo_config.source {
        RepoSource::Deb => {
            crate::package::fetch::fetch_deb_to_temp(name, &repo_name, &repo_config, &raw_dir)
        }
        RepoSource::Rpm => {
            crate::package::fetch::fetch_rpm_to_temp(name, &repo_name, &repo_config, &raw_dir)
        }
        RepoSource::Native => {
            crate::package::fetch::fetch_native_to_temp(name, &repo_name, &repo_config, &raw_dir)
        }
    };

    let fetched = match result {
        Ok(f) => f,
        Err(_) => {
            return Err(SpmError::package_not_found(format!(
                "Package '{name}' not found after fetch"
            )));
        }
    };

    spinner.message("Verifying and registering...");
    let hash = crate::util::hash::hash_dir(Path::new(&fetched.extracted_dir))?;

    db::with_write_lock(|conn| {
        if db::is_installed_for_user(conn, user_id, name)? {
            spinner.finish();
            return Err(SpmError::package_already_installed(format!(
                "Package '{name}' is already installed for user {user_id}"
            )));
        }

        let (pkg_dir, is_new) = crate::cache::copy_to_shared_cache(
            Path::new(&fetched.extracted_dir),
            &hash,
        )?;

        spinner.message("Creating symlinks...");
        let symlinks = crate::cache::create_user_symlinks(&hash, user_home)?;

        if symlinks.is_empty() {
            if is_new {
                let _ = fs::remove_dir_all(&pkg_dir);
            }
            spinner.finish();
            return Err(SpmError::other(format!(
                "No executable files found in package '{name}'"
            )));
        }

        db::record_user_install(conn, user_id, name, &source_to_format(&repo_config.source), &hash)?;
        spinner.finish();
        crate::output::step_success(format!("Installed '{}' for {} ({} symlinks)", name, user_name, symlinks.len()));

        Ok(())
    })?;

    Ok(())
}

fn collect_symlink_targets(pkg_dir: &Path) -> Vec<String> {
    let mut targets = Vec::new();
    if pkg_dir.exists() {
        for subdir in &["usr/bin", "usr/local/bin", "bin"] {
            let src_dir = pkg_dir.join(subdir);
            if src_dir.is_dir() {
                for e in fs::read_dir(&src_dir).into_iter().flatten().flatten() {
                    targets.push(e.file_name().to_string_lossy().to_string());
                }
            }
        }
    }
    targets
}

pub fn remove_for_user(name: &str, user_id: u32, user_home: &str) -> SpmResult<()> {
    let user_name = crate::util::user::resolve_user_name(user_id).unwrap_or_else(|| user_id.to_string());
    crate::output::section(format!("🗑 Removing '{name}' for {user_name}"));

    // Phase 1: Collect info and verify package exists (read lock)
    let (ui, hash, symlink_targets) = db::with_read_lock(|conn| {
        let installs = db::list_user_installs(conn, user_id)?;
        let ui = installs.iter()
            .find(|i| i.package_name == name)
            .cloned()
            .ok_or_else(|| SpmError::package_not_found(format!(
                "Package '{name}' is not installed for user {user_id}"
            )))?;

        let hash = ui.package_hash.clone();

        // Collect symlink targets before any changes
        let symlink_targets: Vec<String> = collect_symlink_targets(&crate::cache::shared_package_dir(&hash));

        Ok((ui, hash, symlink_targets))
    })?;

    let mut spinner = crate::output::Spinner::new(format!("Removing symlinks for '{name}' ({})", symlink_targets.len()));

    // Phase 2: Physically remove symlinks
    let bin_dir = crate::cache::user_bin_dir(user_home);
    let mut remove_errors: Vec<String> = Vec::new();
    for target in &symlink_targets {
        let dest = bin_dir.join(target);
        if dest.exists() || dest.is_symlink() {
            if let Err(e) = fs::remove_file(&dest) {
                remove_errors.push(format!("  {}: {e}", dest.display()));
            } else {
                spinner.message(&format!("Removed {target}"));
            }
        }
    }

    // Phase 3: Remove shared package if no remaining users
    let remaining = db::with_read_lock(|conn| {
        db::count_users_for_package_hash(conn, &hash)
    })?;
    if remaining <= 1 {
        spinner.message("Removing shared cache...");
        if let Err(e) = crate::cache::remove_shared_package(&hash) {
            remove_errors.push(format!("  shared package {hash}: {e}"));
        }
    }

    // Phase 4: Verify 100% physical removal before DB delete
    if !remove_errors.is_empty() {
        spinner.finish();
        return Err(SpmError::other(format!(
            "Failed to remove some symlinks for '{name}' (user {user_name}). DB was NOT modified:\n{}",
            remove_errors.join("\n"),
        )));
    }

    // Phase 5: Safe to update DB
    db::with_write_lock(|conn| {
        db::remove_user_install(conn, user_id, name, &ui.package_format)?;
        spinner.finish();
        crate::output::step_success(format!("Removed '{name}' for {user_name}"));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_collect_symlink_targets_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let targets = collect_symlink_targets(dir.path());
        assert!(targets.is_empty());
    }

    #[test]
    fn test_collect_symlink_targets_usr_bin() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("usr/bin")).unwrap();
        fs::write(dir.path().join("usr/bin/hello"), b"").unwrap();
        fs::write(dir.path().join("usr/bin/world"), b"").unwrap();

        let mut targets = collect_symlink_targets(dir.path());
        targets.sort();
        assert_eq!(targets, vec!["hello", "world"]);
    }

    #[test]
    fn test_collect_symlink_targets_usr_local_bin() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("usr/local/bin")).unwrap();
        fs::write(dir.path().join("usr/local/bin/tool"), b"").unwrap();

        let targets = collect_symlink_targets(dir.path());
        assert_eq!(targets, vec!["tool"]);
    }

    #[test]
    fn test_collect_symlink_targets_bin() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("bin")).unwrap();
        fs::write(dir.path().join("bin/run"), b"").unwrap();

        let targets = collect_symlink_targets(dir.path());
        assert_eq!(targets, vec!["run"]);
    }

    #[test]
    fn test_collect_symlink_targets_multiple_dirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("usr/bin")).unwrap();
        fs::create_dir_all(dir.path().join("bin")).unwrap();
        fs::write(dir.path().join("usr/bin/a"), b"").unwrap();
        fs::write(dir.path().join("bin/b"), b"").unwrap();

        let mut targets = collect_symlink_targets(dir.path());
        targets.sort();
        assert_eq!(targets, vec!["a", "b"]);
    }

    #[test]
    fn test_collect_symlink_targets_ignores_non_bin_dirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("usr/lib")).unwrap();
        fs::write(dir.path().join("usr/lib/libfoo.so"), b"").unwrap();
        fs::create_dir_all(dir.path().join("usr/bin")).unwrap();
        fs::write(dir.path().join("usr/bin/prog"), b"").unwrap();

        let targets = collect_symlink_targets(dir.path());
        assert_eq!(targets, vec!["prog"]);
    }

    #[test]
    fn test_collect_symlink_targets_nonexistent_dir() {
        let targets = collect_symlink_targets(Path::new("/nonexistent-dir"));
        assert!(targets.is_empty());
    }
}
