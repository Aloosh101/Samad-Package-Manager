use std::fs;
use std::path::Path;
use std::process::Command;

use chrono::Utc;
use rayon::prelude::*;

use crate::config::paths;
use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::types::*;
use crate::verify;

pub(super) fn install_sandboxed(name: &str, level: &str, conn: &rusqlite::Connection, yes: bool) -> SpmResult<()> {
    let sandbox_dir = paths::sandbox_dir(name);
    fs::create_dir_all(&sandbox_dir)?;

    let tmp_dir_obj = tempfile::tempdir()
        .map_err(|e| SpmError::other(format!("Failed to create temp directory: {e}")))?;
    let tmp_dir = tmp_dir_obj.path().to_string_lossy().to_string();
    let sandbox_dir_str = sandbox_dir.to_string_lossy();

    let pid = PackageId::new(name, PackageFormat::Sam);
    let resolved = crate::package::resolver::resolve_dependencies(&pid, Default::default())?;
    let install_order = resolved.topological_order.clone();

    if !resolved.cycles.is_empty() {
        crate::output::step_warn(format!("Dependency cycles detected: {:?}", resolved.cycles));
    }

    let to_install: Vec<&PackageId> = install_order.iter()
        .filter(|p| p.name == name || !db::is_installed(conn, &p.name))
        .collect();

    let all_names: Vec<&str> = install_order.iter().map(|p| p.name.as_str()).collect();
    if !verify::prompt_before_install(&[name], &all_names, yes)? {
        return Err(SpmError::other(format!("Installation of '{name}' cancelled.")));
    }

    crate::output::step_info(format!("Downloading {} packages for sandbox...", to_install.len()));

    let download_results: Vec<(String, SpmResult<()>)> = to_install.par_iter().map(|pkg_id| {
        let pkg_tmp = format!("{}/{}", tmp_dir, pkg_id.name);
        let _ = fs::create_dir_all(&pkg_tmp);
        let result = sandbox_download_package(&pkg_id.name, &pkg_tmp);
        (pkg_id.name.clone(), result)
    }).collect();

    for (pkg_name, result) in &download_results {
        if let Err(e) = result {
            tracing::warn!("Failed to download dependency '{pkg_name}' for sandbox: {e}");
            return Err(SpmError::other(format!(
                "Failed to download dependency '{pkg_name}': {e}"
            )));
        }
    }

    for pkg_id in &install_order {
        if pkg_id.name != name && db::is_installed(conn, &pkg_id.name) {
            continue;
        }

        let pkg_tmp = format!("{}/{}", tmp_dir, pkg_id.name);

        sandbox_extract_package(&pkg_id.name, &pkg_tmp, sandbox_dir_str.as_ref())?;
    }

    match level {
        "strict" => {
            check_missing_libraries(sandbox_dir_str.as_ref())?;
            crate::output::step_success(format!("Sandbox '{}' set up (Strict: no system links)", name));
        }
        "full" => {
            setup_symlink_sandbox(name, sandbox_dir_str.as_ref())?;
        }
        _ => {}
    }

    // Extract scripts before DB commit (but don't run them yet)
    let run_scripts = level != "strict";
    let scripts_list: Vec<(String, crate::package::scripts::Scripts)> = if run_scripts {
        install_order.iter()
            .filter(|p| p.name == name || !db::is_installed(conn, &p.name))
            .filter_map(|pkg_id| {
                let pkg_tmp = format!("{}/{}", tmp_dir, pkg_id.name);
                extract_sandbox_scripts(&pkg_id.name, &pkg_tmp).ok().map(|s| (pkg_id.name.clone(), s))
            })
            .collect()
    } else {
        Vec::new()
    };

    let pkg_hash = match crate::util::hash::hash_dir(&sandbox_dir) {
        Ok(h) => {
            let _ = crate::package::store::copy_to_store(&sandbox_dir, &h);
            Some(h)
        }
        Err(e) => {
            tracing::warn!("Failed to hash sandbox directory for '{name}': {e}");
            None
        }
    };

    // Atomic DB commit
    conn.execute_batch("BEGIN")?;
    let result = (|| -> SpmResult<()> {
        let tx = Transaction {
            id: None,
            action: TransactionAction::Install,
            timestamp: Utc::now().to_rfc3339(),
            user: crate::util::fs::whoami(),
            status: TransactionStatus::Completed,
            packages: vec![name.to_string()],
            snapshot_id: None,
        };
        let tx_id = db::record_transaction(conn, &tx)?;

        let file_records = scan_sandbox_files(&sandbox_dir, name, tx_id)?;
        if !file_records.is_empty() {
            db::record_files(conn, &file_records)?;
        }

        let pkg = InstalledPackage {
            name: name.to_string(),
            version: String::new(),
            format: PackageFormat::Sam,
            install_type: InstallType::Sandbox,
            manifest: None,
            install_date: Utc::now().to_rfc3339(),
            source_repo: None,
            store_hash: pkg_hash,
            origin: InstallOrigin::Spm,
        };
        db::add_installed_package(conn, &pkg)?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            // Run scripts AFTER commit — inside chroot into sandbox dir
            for (pkg_name, scripts) in &scripts_list {
                if let Some(ref preinst) = scripts.preinst {
                    crate::output::step_info(format!("Running preinst for '{}' in sandbox", pkg_name));
                    let _ = crate::package::scripts::run_script_in_sandbox(preinst, "install", sandbox_dir_str.as_ref());
                }
                if let Some(ref postinst) = scripts.postinst {
                    crate::output::step_info(format!("Running postinst for '{}' in sandbox", pkg_name));
                    let _ = crate::package::scripts::run_script_in_sandbox(postinst, "configure", sandbox_dir_str.as_ref());
                }
            }

            let _ = crate::sandbox::desktop::create_sandbox_desktop_entries(name, &sandbox_dir);

            crate::output::step_success(format!("Sandbox '{}' set up ({level})", name));
            Ok(())
        }
        Err(e) => {
            conn.execute_batch("ROLLBACK")?;
            Err(e)
        }
    }
}

fn extract_sandbox_scripts(_name: &str, pkg_tmp: &str) -> SpmResult<crate::package::scripts::Scripts> {
    let entries = match fs::read_dir(pkg_tmp) {
        Ok(e) => e,
        Err(_) => return Ok(crate::package::scripts::Scripts::default()),
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let path_str = p.to_string_lossy().to_string();
        if p.extension().and_then(|e| e.to_str()) == Some("deb") {
            let tmp_scripts = tempfile::tempdir()
                .map_err(|e| SpmError::other(format!("Failed to create temp dir: {e}")))?;
            let scripts_dir = tmp_scripts.path().to_string_lossy().to_string();
            return crate::package::scripts::extract_deb_scripts(&path_str, &scripts_dir);
        }
        if p.extension().and_then(|e| e.to_str()) == Some("rpm") {
            return crate::package::scripts::extract_rpm_scripts(&path_str);
        }
        if p.extension().and_then(|e| e.to_str()) == Some("sam") {
            let tmp_scripts = tempfile::tempdir()
                .map_err(|e| SpmError::other(format!("Failed to create temp dir: {e}")))?;
            let scripts_dir = tmp_scripts.path().to_string_lossy().to_string();
            return crate::package::scripts::extract_sam_scripts(&path_str, &scripts_dir);
        }
    }
    Ok(crate::package::scripts::Scripts::default())
}

fn scan_sandbox_files(sandbox_dir: &Path, _name: &str, tx_id: i64) -> SpmResult<Vec<FileRecord>> {
    let mut records = Vec::new();
    if !sandbox_dir.exists() {
        return Ok(records);
    }
    for entry in walkdir::WalkDir::new(sandbox_dir).min_depth(1) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }
        let abs_path = entry.path();
        records.push(FileRecord {
            id: None,
            transaction_id: tx_id,
            package: "".to_string(),
            format: PackageFormat::Sam,
            filepath: abs_path.to_string_lossy().to_string(),
            hash: crate::util::hash::hash_file(&abs_path.to_string_lossy()).unwrap_or_default(),
            action: FileAction::Created,
        });
    }
    Ok(records)
}

fn sandbox_download_package(name: &str, pkg_tmp: &str) -> SpmResult<()> {
    use crate::config::repos;
    let repos_list = repos::load_repos()?;
    for (_repo_name, repo_config) in &repos_list {
        let ok = match repo_config.source {
            RepoSource::Apt => {
                Command::new(crate::util::backend::resolve("apt-get"))
                    .args(["download", name, "-o", &format!("Dir::Cache={}", pkg_tmp)])
                    .output()
                    .ok()
                    .is_some_and(|o| o.status.success())
            }
            RepoSource::Dnf => {
                Command::new(crate::util::backend::resolve("dnf"))
                    .args(["download", "--destdir", pkg_tmp, name])
                    .output()
                    .ok()
                    .is_some_and(|o| o.status.success())
            }
            _ => false,
        };
        if ok {
            return Ok(());
        }
    }
    Err(SpmError::package_not_found(format!(
        "Failed to download '{name}' for sandbox install"
    )))
}

fn sandbox_extract_package(name: &str, pkg_tmp: &str, sandbox_dir: &str) -> SpmResult<()> {
    if let Ok(entries) = fs::read_dir(pkg_tmp) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("deb") {
                return crate::package::deb::extract_deb(&p.to_string_lossy(), sandbox_dir);
            }
            if p.extension().and_then(|e| e.to_str()) == Some("rpm") {
                return crate::package::rpm::extract_rpm(&p.to_string_lossy(), sandbox_dir);
            }
        }
    }
    tracing::debug!("No package file found in {pkg_tmp} for '{name}' — possibly a native dep already handled");
    Ok(())
}

fn check_missing_libraries(sandbox_dir: &str) -> SpmResult<()> {
    let sandbox = Path::new(sandbox_dir);

    let elf_files: Vec<_> = walkdir::WalkDir::new(sandbox)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .filter(|p| crate::util::fs::is_elf(p))
        .collect();

    let missing = std::thread::scope(|s| {
        let mut handles = Vec::new();
        for chunk in elf_files.chunks(8) {
            let owned: Vec<_> = chunk.to_vec();
            handles.push(s.spawn(move || {
                let mut local_missing = Vec::new();
                for path in &owned {
                    let output = match Command::new("ldd").arg(path).output() {
                        Ok(o) => o,
                        Err(_) => continue,
                    };
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if stderr.contains("not a dynamic executable") {
                        continue;
                    }
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines() {
                        if line.contains("not found") {
                            let lib = line.split_whitespace().next().unwrap_or(line);
                            let relative = path.strip_prefix(sandbox_dir).unwrap_or(path);
                            local_missing.push(format!("  {} requires {} (not found)", relative.display(), lib));
                        }
                    }
                }
                local_missing
            }));
        }
        let mut all_missing = Vec::new();
        for h in handles {
            all_missing.extend(h.join().unwrap());
        }
        all_missing
    });

    if missing.is_empty() {
        Ok(())
    } else {
        let details = missing.join("\n");
        Err(SpmError::sandbox(format!(
            "Strict sandbox: missing shared libraries:\n{details}\n\
             Use --sandbox=full to allow system library symlinks.",
        )))
    }
}

fn setup_symlink_sandbox(name: &str, sandbox_dir: &str) -> SpmResult<()> {
    let system_libs = [
        "/usr/lib",
        "/usr/lib64",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
    ];

    let sandbox_lib_dir = format!("{}/usr/lib", sandbox_dir);
    fs::create_dir_all(&sandbox_lib_dir)?;

    for lib_dir in &system_libs {
        let system_path = Path::new(lib_dir);
        if system_path.exists() {
            let entries = fs::read_dir(system_path)?;
            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                if let Some(name_str) = path.file_name().and_then(|n| n.to_str()) {
                    if name_str.contains(".so") {
                        let link_path = Path::new(&sandbox_lib_dir).join(name_str);
                        if !link_path.exists() {
                            if let Err(e) = std::os::unix::fs::symlink(&path, &link_path) {
                                tracing::warn!("Failed to create symlink {} -> {}: {}",
                                    link_path.display(), path.display(), e);
                            }
                        }
                    }
                }
            }
        }
    }

    crate::output::step_success(format!("Sandbox '{}' set up (Level 1: symlink farm)", name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_scan_sandbox_files_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let records = scan_sandbox_files(dir.path(), "test-pkg", 1).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_scan_sandbox_files_with_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("foo"), b"content").unwrap();
        fs::write(dir.path().join("bar"), b"data").unwrap();

        let records = scan_sandbox_files(dir.path(), "test-pkg", 42).unwrap();
        assert_eq!(records.len(), 2);

        for r in &records {
            assert_eq!(r.transaction_id, 42);
            assert_eq!(r.package, "");
            assert_eq!(r.format, PackageFormat::Sam);
            assert_eq!(r.action, FileAction::Created);
        }
    }

    #[test]
    fn test_scan_sandbox_files_skips_dirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();
        fs::write(dir.path().join("subdir").join("f1"), b"a").unwrap();
        fs::write(dir.path().join("f2"), b"b").unwrap();

        let records = scan_sandbox_files(dir.path(), "test-pkg", 0).unwrap();
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|r| r.action == FileAction::Created));
    }

    #[test]
    fn test_scan_sandbox_files_hash_is_present() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("hello.txt"), b"hello world").unwrap();
        let records = scan_sandbox_files(dir.path(), "test-pkg", 1).unwrap();
        assert_eq!(records.len(), 1);
        assert!(!records[0].hash.is_empty());
    }

    #[test]
    fn test_scan_sandbox_files_nonexistent_dir() {
        let records = scan_sandbox_files(Path::new("/nonexistent-sandbox-dir"), "test-pkg", 1).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_scan_sandbox_files_multiple_transactions() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"a").unwrap();

        let r1 = scan_sandbox_files(dir.path(), "pkg1", 10).unwrap();
        let r2 = scan_sandbox_files(dir.path(), "pkg2", 20).unwrap();

        assert_eq!(r1.len(), 1);
        assert_eq!(r2.len(), 1);
        assert_eq!(r1[0].transaction_id, 10);
        assert_eq!(r2[0].transaction_id, 20);
    }
}
