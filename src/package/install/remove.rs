use std::fs;
use std::path::Path;

use chrono::Utc;

use crate::config::paths;
use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::package::hooks;
use crate::types::*;

fn create_remove_transaction(pid: &PackageId) -> Transaction {
    Transaction {
        id: None,
        action: TransactionAction::Remove,
        timestamp: Utc::now().to_rfc3339(),
        user: crate::util::fs::whoami(),
        status: TransactionStatus::Completed,
        packages: vec![pid.to_string()],
        snapshot_id: None,
    }
}

fn format_remove_failed_msg(name: &str, action: &str, errors: &[String]) -> SpmError {
    SpmError::other(format!(
        "Failed to {action} some files for '{name}'. DB was NOT modified:\n{}",
        errors.join("\n"),
    ))
}

/// Check if a filepath is a conffile — either explicitly listed or under /etc/
fn is_conffile(path: &str, conffiles: &[String]) -> bool {
    if conffiles.iter().any(|c| c == path) {
        return true;
    }
    // Traditional convention: any file under /etc/ is a conffile
    path.starts_with("/etc/")
}

fn dirs_to_cleanup(name: &str, file_records: &[FileRecord]) -> Vec<String> {
    let mut dirs = Vec::new();
    for d in &[format!("/etc/{}", name), format!("/var/lib/{}", name)] {
        if file_records.iter().any(|r| r.filepath.starts_with(&*d)) {
            dirs.push(d.clone());
        }
    }
    dirs
}

pub fn remove_package(name: &str) -> SpmResult<()> {
    crate::output::section(format!("🗑 Removing {}", name));
    let mut spinner = crate::output::Spinner::new(format!("Preparing to remove '{name}'..."));

    // Phase 1: Read package info (read lock)
    let (pkg, file_records, scripts) = db::with_read_lock(|conn| {
        let pkg = db::get_installed_package(conn, name)?
            .ok_or_else(|| SpmError::package_not_found(format!("Package '{name}' is not installed")))?;
        let file_records = db::get_files_by_package(conn, name)?;
        let scripts = crate::package::scripts::load_scripts(name).unwrap_or_default();
        Ok((pkg, file_records, scripts))
    })?;

    let pid = PackageId::new(name, pkg.format.clone());

    // Phase 2: Run prerm script (before any changes)
    if let Some(ref script) = scripts.prerm {
        crate::output::step_info(format!("Running prerm script for {name}"));
        let _ = crate::package::scripts::run_script(script, "remove");
    }

    // Determine conffiles from manifest — files under /etc/ or explicitly listed
    let conffiles: Vec<String> = pkg.manifest.as_ref()
        .and_then(|m| serde_json::from_str::<Manifest>(m).ok())
        .map(|m| m.conffiles)
        .unwrap_or_default();

    // Phase 3: Physically remove files (skip conffiles — they persist on remove)
    spinner.message(&format!("Removing {} files", file_records.len()));
    let mut remove_errors: Vec<String> = Vec::new();
    for f in &file_records {
        if is_conffile(&f.filepath, &conffiles) {
            tracing::debug!("Preserving conffile on remove: {}", f.filepath);
            continue;
        }
        let path = Path::new(&f.filepath);
        if path.exists() || path.is_symlink() {
            match fs::remove_file(path) {
                Ok(()) => {
                    if let Some(parent) = path.parent() {
                        let _ = fs::remove_dir(parent);
                    }
                    spinner.message(&f.filepath);
                }
                Err(e) => {
                    remove_errors.push(format!("  {}: {e}", f.filepath));
                }
            }
        }
    }

    // Phase 4: Remove sandbox directory if applicable
    if matches!(pkg.install_type, InstallType::Sandbox) {
        spinner.message("Removing sandbox...");
        let _ = crate::sandbox::desktop::remove_sandbox_desktop_entries(name);

        let sandbox_dir = paths::sandbox_dir(name);
        if sandbox_dir.exists() {
            if let Err(e) = fs::remove_dir_all(&sandbox_dir) {
                remove_errors.push(format!("  sandbox/{}: {e}", sandbox_dir.display()));
            }
        }
    }

    // Phase 5: Verify 100% physical deletion before touching DB
    if !remove_errors.is_empty() {
        spinner.finish();
        return Err(format_remove_failed_msg(name, "remove", &remove_errors));
    }

    // Phase 6: Now it's safe to update the DB
    db::with_write_lock(|conn| {
        let tx = create_remove_transaction(&pid);
        db::record_transaction(conn, &tx)?;
        db::remove_installed_package_by_id(conn, &pid)?;

        if let Some(ref hash) = pkg.store_hash {
            let origin = crate::package::store::origin_from_format(&pkg.format);
            let _ = crate::package::store::gc_store_with_origin(conn, hash, origin);
        }

        Ok(())
    })?;

    // Phase 7: SAM v2 cleanup (disable systemd units, remove sysusers/tmpfiles confs)
    if let Some(ref m) = pkg.manifest {
        if let Ok(manifest) = serde_json::from_str::<Manifest>(m) {
            hooks::remove_sam_v2_hooks(&manifest);
        }
    }

    // Phase 8: Run postrm script
    if let Some(ref script) = scripts.postrm {
        crate::output::step_info(format!("Running postrm script for {name}"));
        let _ = crate::package::scripts::run_script(script, "remove");
    }

    // Phase 9: Clean up triggers and run remove triggers
    crate::package::triggers::remove_triggers(name);
    crate::package::triggers::run_triggers("remove", &[name.to_string()]);

    crate::output::remove_message(name, std::time::Duration::from_secs(0));
    Ok(())
}

pub fn purge_package(name: &str) -> SpmResult<()> {
    crate::output::section(format!("🧹 Purging {}", name));

    // Phase 1: Verify package exists and collect info (read lock)
    let (pkg, file_records, scripts) = db::with_read_lock(|conn| {
        let pkg = db::get_installed_package(conn, name)?
            .ok_or_else(|| SpmError::package_not_found(format!("Package '{name}' is not installed")))?;
        let file_records = db::get_files_by_package(conn, name)?;
        let scripts = crate::package::scripts::load_scripts(name).unwrap_or_default();
        Ok((pkg, file_records, scripts))
    })?;

    let pid = PackageId::new(name, pkg.format.clone());

    // Phase 2: Run prerm script
    if let Some(ref script) = scripts.prerm {
        crate::output::step_info(format!("Running prerm script for {name}"));
        let _ = crate::package::scripts::run_script(script, "remove");
    }

    // Phase 3: Physically remove ALL files (package files + config + data + sandbox + scripts)
    let mut remove_errors: Vec<String> = Vec::new();

    // 3a: Package files
    for f in &file_records {
        let path = Path::new(&f.filepath);
        if path.exists() || path.is_symlink() {
            if let Err(e) = fs::remove_file(path) {
                remove_errors.push(format!("  {}: {e}", f.filepath));
            }
        }
    }

    // 3b: Config and data directories — only if SPM-managed
    for d in &dirs_to_cleanup(name, &file_records) {
        let p = Path::new(d);
        if !p.exists() {
            continue;
        }
        if let Err(e) = fs::remove_dir_all(p) {
            remove_errors.push(format!("  {d}: {e}"));
        }
    }

    // 3c: Sandbox desktop entries
    let _ = crate::sandbox::desktop::remove_sandbox_desktop_entries(name);

    // 3d: Sandbox directory
    let sandbox_dir = paths::sandbox_dir(name);
    if sandbox_dir.exists() {
        if let Err(e) = fs::remove_dir_all(&sandbox_dir) {
            remove_errors.push(format!("  sandbox: {e}"));
        }
    }

    // 3e: Scripts directory
    crate::package::scripts::remove_scripts(name).unwrap_or_else(|e| {
        remove_errors.push(format!("  scripts: {e}"));
    });

    // Phase 4: Verify 100% deletion before touching DB
    if !remove_errors.is_empty() {
        return Err(format_remove_failed_msg(name, "purge", &remove_errors));
    }

    // Phase 5: Safe to update DB
    db::with_write_lock(|conn| {
        let tx = create_remove_transaction(&pid);
        db::record_transaction(conn, &tx)?;
        db::remove_installed_package_by_id(conn, &pid)?;

        if let Some(ref hash) = pkg.store_hash {
            let origin = crate::package::store::origin_from_format(&pkg.format);
            let _ = crate::package::store::gc_store_with_origin(conn, hash, origin);
        }
        crate::package::cleanup::gc_cache_for_package(name)?;

        Ok(())
    })?;

    // Phase 6: SAM v2 cleanup (disable systemd units, remove sysusers/tmpfiles confs)
    if let Some(ref m) = pkg.manifest {
        if let Ok(manifest) = serde_json::from_str::<Manifest>(m) {
            hooks::remove_sam_v2_hooks(&manifest);
        }
    }

    // Phase 7: Postrm
    if let Some(ref script) = scripts.postrm {
        crate::output::step_info(format!("Running postrm script for {name}"));
        let _ = crate::package::scripts::run_script(script, "remove");
    }

    // Phase 8: Clean up triggers and run remove triggers
    crate::package::triggers::remove_triggers(name);
    crate::package::triggers::run_triggers("remove", &[name.to_string()]);

    crate::output::remove_message(name, std::time::Duration::from_secs(0));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_remove_transaction_fields() {
        let pid = PackageId::new("test-pkg", PackageFormat::Deb);
        let tx = create_remove_transaction(&pid);

        assert!(matches!(tx.action, TransactionAction::Remove));
        assert!(tx.packages.contains(&"test-pkg:deb".to_string()));
        assert!(matches!(tx.status, TransactionStatus::Completed));
        assert!(tx.snapshot_id.is_none());
    }

    #[test]
    fn test_create_remove_transaction_sam_package() {
        let pid = PackageId::new("myapp", PackageFormat::Sam);
        let tx = create_remove_transaction(&pid);

        assert_eq!(tx.packages, vec!["myapp:sam"]);
    }

    #[test]
    fn test_format_remove_failed_msg_empty_errors() {
        let err = format_remove_failed_msg("pkg", "remove", &[]);
        let msg = format!("{}", err);
        assert!(msg.contains("Failed to remove some files for 'pkg'"));
        assert!(msg.contains("DB was NOT modified"));
    }

    #[test]
    fn test_format_remove_failed_msg_with_errors() {
        let err = format_remove_failed_msg("pkg", "purge", &[
            "  /usr/bin/foo: Permission denied".into(),
            "  /etc/pkg/conf: Read-only".into(),
        ]);
        let msg = format!("{}", err);
        assert!(msg.contains("Failed to purge some files for 'pkg'"));
        assert!(msg.contains("Permission denied"));
        assert!(msg.contains("Read-only"));
    }

    #[test]
    fn test_dirs_to_cleanup_matching() {
        let records = vec![
            FileRecord { id: None, transaction_id: 0, package: "myapp".into(), format: PackageFormat::Deb, filepath: "/etc/myapp/conf".into(), hash: "abc".into(), action: FileAction::Created },
            FileRecord { id: None, transaction_id: 0, package: "myapp".into(), format: PackageFormat::Deb, filepath: "/var/lib/myapp/data".into(), hash: "def".into(), action: FileAction::Created },
            FileRecord { id: None, transaction_id: 0, package: "myapp".into(), format: PackageFormat::Deb, filepath: "/usr/bin/myapp".into(), hash: "ghi".into(), action: FileAction::Created },
        ];
        let dirs = dirs_to_cleanup("myapp", &records);
        assert!(dirs.contains(&"/etc/myapp".to_string()));
        assert!(dirs.contains(&"/var/lib/myapp".to_string()));
    }

    #[test]
    fn test_dirs_to_cleanup_no_match() {
        let records = vec![
            FileRecord { id: None, transaction_id: 0, package: "app".into(), format: PackageFormat::Deb, filepath: "/usr/bin/app".into(), hash: "abc".into(), action: FileAction::Created },
        ];
        let dirs = dirs_to_cleanup("app", &records);
        assert!(dirs.is_empty());
    }

    #[test]
    fn test_dirs_to_cleanup_partial_match() {
        let records = vec![
            FileRecord { id: None, transaction_id: 0, package: "myapp".into(), format: PackageFormat::Deb, filepath: "/etc/myapp/conf".into(), hash: "abc".into(), action: FileAction::Created },
        ];
        let dirs = dirs_to_cleanup("myapp", &records);
        assert_eq!(dirs.len(), 1);
        assert!(dirs.contains(&"/etc/myapp".to_string()));
    }

    // ── is_conffile tests ──

    #[test]
    fn test_is_conffile_explicit_list() {
        let conffiles = vec!["/etc/myapp/custom.conf".to_string()];
        assert!(is_conffile("/etc/myapp/custom.conf", &conffiles));
    }

    #[test]
    fn test_is_conffile_etc_prefix() {
        let conffiles: Vec<String> = vec![];
        assert!(is_conffile("/etc/myapp/nginx.conf", &conffiles));
    }

    #[test]
    fn test_is_conffile_not_under_etc() {
        let conffiles: Vec<String> = vec![];
        assert!(!is_conffile("/usr/bin/myapp", &conffiles));
    }

    #[test]
    fn test_is_conffile_explicit_overrides_empty_list() {
        let conffiles = vec!["/opt/myapp/config.yml".to_string()];
        assert!(is_conffile("/opt/myapp/config.yml", &conffiles));
        assert!(!is_conffile("/opt/myapp/other.yml", &conffiles));
    }

    #[test]
    fn test_is_conffile_slash_etc_only() {
        let conffiles: Vec<String> = vec![];
        assert!(!is_conffile("/etcetc/test", &conffiles));
        assert!(!is_conffile("/usr/etc/test", &conffiles));
    }
}
