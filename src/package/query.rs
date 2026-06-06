use chrono::Utc;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::{paths, repos};
use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::types::*;

pub fn match_native_packages(query: &str, index: &RepoIndex) -> Vec<Package> {
    let q = query.to_lowercase();
    let mut results = Vec::new();
    for pkg in &index.packages {
        if pkg.name.to_lowercase().contains(&q)
            || pkg.description.to_lowercase().contains(&q)
        {
            results.push(Package {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                format: PackageFormat::Sam,
                source_repo: Some("native".to_string()),
                ..Default::default()
            });
        }
    }
    results
}

pub fn search_packages(query: &str) -> SpmResult<Vec<Package>> {
    let mut results = Vec::new();
    let repos_list = repos::load_repos()?;
    for (_repo_name, repo_config) in &repos_list {
        match repo_config.source {
            RepoSource::Apt => {
                if let Ok(output) = Command::new(crate::util::backend::resolve("apt-cache"))
                    .args(["search", query])
                    .stderr(Stdio::null())
                    .output()
                {
                    if output.status.success() {
                        for line in String::from_utf8_lossy(&output.stdout).lines() {
                            if let Some((name, _desc)) = line.split_once(" - ") {
                                results.push(Package {
                                    name: name.trim().to_string(),
                                    version: String::new(),
                                    format: PackageFormat::Deb,
                                    source_repo: Some("apt".to_string()),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                }
            }
            RepoSource::Dnf => {
                if let Ok(output) = Command::new(crate::util::backend::resolve("dnf"))
                    .args(["search", "--quiet", query])
                    .stderr(Stdio::null())
                    .output()
                {
                    if output.status.success() {
                        for line in String::from_utf8_lossy(&output.stdout).lines() {
                            if let Some((pkg, _desc)) = line.split_once(" : ") {
                                if let Some(name) = pkg.rsplit_once('.').map(|(n, _a)| n).or(Some(pkg)) {
                                    results.push(Package {
                                        name: name.trim().to_string(),
                                        version: String::new(),
                                        format: PackageFormat::Rpm,
                                        source_repo: Some("dnf".to_string()),
                                        ..Default::default()
                                    });
                                }
                            }
                        }
                    }
                }
            }
            RepoSource::Native => {
                let repo_cache = paths::repos_cache_dir().join("native").join(_repo_name).join("repo-index.json");
                if repo_cache.exists() {
                    if let Ok(content) = fs::read_to_string(&repo_cache) {
                        if let Ok(index) = serde_json::from_str::<crate::types::RepoIndex>(&content) {
                            results.extend(match_native_packages(query, &index));
                        }
                    }
                }
            }
        }
    }
    results.sort_by(|a, b| a.name.cmp(&b.name));
    results.dedup_by(|a, b| a.name == b.name);
    Ok(results)
}

pub fn package_info(name: &str) -> SpmResult<String> {
    let conn = db::get_connection()?;
    if let Some(pkg) = db::get_installed_package(&conn, name)? {
        return Ok(format!(
            "Package: {}\nVersion: {}\nFormat: {:?}\nType: {:?}\nInstalled: {}",
            pkg.name, pkg.version, pkg.format, pkg.install_type, pkg.install_date
        ));
    }
    let repos_list = repos::load_repos()?;
    for (_repo_name, repo_config) in &repos_list {
        match repo_config.source {
            RepoSource::Apt => {
                if let Ok(output) = Command::new(crate::util::backend::resolve("apt-cache")).args(["show", name]).stderr(Stdio::null()).output() {
                    if output.status.success() {
                        let out = String::from_utf8_lossy(&output.stdout);
                        if !out.trim().is_empty() {
                            return Ok(out.to_string());
                        }
                    }
                }
            }
            RepoSource::Dnf => {
                if let Ok(output) = Command::new(crate::util::backend::resolve("dnf")).args(["repoquery", "--info", name]).stderr(Stdio::null()).output() {
                    if output.status.success() {
                        let out = String::from_utf8_lossy(&output.stdout);
                        if !out.trim().is_empty() {
                            return Ok(out.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Err(SpmError::package_not_found(format!("Package '{name}' not found in any repository")))
}

pub fn list_package_files(name: &str) -> SpmResult<Vec<String>> {
    let conn = db::get_connection()?;
    if db::get_installed_package(&conn, name)?.is_none() {
        return Ok(Vec::new());
    }
    let files = db::get_files_by_package(&conn, name)?;
    if !files.is_empty() {
        return Ok(files.into_iter().map(|f| f.filepath).collect());
    }
    let output = Command::new(crate::util::backend::resolve("dpkg")).args(["-L", name]).output()
        .map_err(|e| SpmError::command_failed(format!("Failed to run dpkg: {e}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).lines().map(|s| s.to_string()).collect());
    }
    let output = Command::new(crate::util::backend::resolve("rpm")).args(["-ql", name]).output()
        .map_err(|e| SpmError::command_failed(format!("Failed to run rpm: {e}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).lines().map(|s| s.to_string()).collect());
    }
    Ok(Vec::new())
}

pub fn package_dependencies(name: &str) -> SpmResult<Vec<Dependency>> {
    // First try SPM DB for installed packages
    if let Ok(conn) = db::get_connection() {
        if let Ok(Some(pkg)) = db::get_installed_package(&conn, name) {
            if let Some(ref manifest_json) = pkg.manifest {
                if let Ok(manifest) = serde_json::from_str::<Manifest>(manifest_json) {
                    if !manifest.dependencies.is_empty() {
                        return Ok(manifest.dependencies);
                    }
                }
            }
        }
    }
    // Fallback: query native backends
    let repos_list = repos::load_repos()?;
    for (_repo_name, repo_config) in &repos_list {
        match repo_config.source {
            RepoSource::Apt => {
                if let Ok(output) = Command::new(crate::util::backend::resolve("apt-cache")).args(["depends", name]).stderr(Stdio::null()).output() {
                    if output.status.success() {
                        return Ok(String::from_utf8_lossy(&output.stdout)
                            .lines()
                            .filter(|l| l.contains("Depends:"))
                            .filter_map(|l| l.split(':').nth(1).map(|s| Dependency {
                                name: s.trim().to_string(),
                                version: String::new(),
                                source: DependencySource::System,
                                format: Some(PackageFormat::Deb),
                            }))
                            .collect());
                    }
                }
            }
            RepoSource::Dnf => {
                if let Ok(output) = Command::new(crate::util::backend::resolve("dnf")).args(["repoquery", "--requires", name]).stderr(Stdio::null()).output() {
                    if output.status.success() {
                        return Ok(String::from_utf8_lossy(&output.stdout)
                            .lines()
                            .filter(|l| !l.is_empty())
                            .map(|l| Dependency {
                                name: l.trim().to_string(),
                                version: String::new(),
                                source: DependencySource::System,
                                format: Some(PackageFormat::Rpm),
                            })
                            .collect());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(Vec::new())
}

pub fn reverse_dependencies(name: &str) -> SpmResult<Vec<String>> {
    // First try SPM DB: scan all installed packages' manifests for reverse deps
    let mut results: Vec<String> = Vec::new();
    if let Ok(conn) = db::get_connection() {
        if let Ok(packages) = db::list_installed_packages(&conn) {
            for pkg in &packages {
                if let Some(ref manifest_json) = pkg.manifest {
                    if let Ok(manifest) = serde_json::from_str::<Manifest>(manifest_json) {
                        let depends_on = manifest.dependencies.iter()
                            .any(|d| d.name == name);
                        if depends_on {
                            results.push(pkg.name.clone());
                        }
                    }
                }
            }
        }
    }
    if !results.is_empty() {
        results.sort();
        results.dedup();
        return Ok(results);
    }
    // Fallback: query native backends
    let repos_list = repos::load_repos()?;
    for (_repo_name, repo_config) in &repos_list {
        match repo_config.source {
            RepoSource::Apt => {
                if let Ok(output) = Command::new(crate::util::backend::resolve("apt-cache"))
                    .args(["--installed", "--recurse", "rdepends", name]).output()
                {
                    if output.status.success() {
                        return Ok(String::from_utf8_lossy(&output.stdout)
                            .lines().skip(1).map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()).collect());
                    }
                }
            }
            RepoSource::Dnf => {
                if let Ok(output) = Command::new(crate::util::backend::resolve("dnf")).args(["repoquery", "--whatrequires", name]).output() {
                    if output.status.success() {
                        return Ok(String::from_utf8_lossy(&output.stdout)
                            .lines().map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()).collect());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(Vec::new())
}

/// Find which installed package owns a given file path.
/// Queries the SPM DB first, then falls back to dpkg / rpm.
pub fn search_file_owner(path: &str) -> SpmResult<Vec<String>> {
    // Canonicalize the path
    let resolved = Path::new(path);
    let path_str = resolved.to_string_lossy().to_string();

    // 1. Search SPM DB files table
    if let Ok(conn) = db::get_connection() {
        if let Ok(files) = db::get_packages_for_file(&conn, &path_str) {
            if !files.is_empty() {
                return Ok(files);
            }
        }
        // Also try with resolved canonical path
        if let Ok(canon) = resolved.canonicalize() {
            let canon_str = canon.to_string_lossy().to_string();
            if canon_str != path_str {
                if let Ok(files) = db::get_packages_for_file(&conn, &canon_str) {
                    if !files.is_empty() {
                        return Ok(files);
                    }
                }
            }
        }
    }

    // 2. Try dpkg -S
    let output = Command::new(crate::util::backend::resolve("dpkg"))
        .args(["-S", path])
        .stderr(std::process::Stdio::null())
        .output();
    if let Ok(o) = output {
        if o.status.success() {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let pkgs: Vec<String> = stdout.lines()
                .filter_map(|l| l.split(':').next().map(|s| s.trim().to_string()))
                .collect();
            if !pkgs.is_empty() {
                return Ok(pkgs);
            }
        }
    }

    // 3. Try rpm -qf
    let output = Command::new(crate::util::backend::resolve("rpm"))
        .args(["-qf", "--queryformat", "%{NAME}\n", path])
        .stderr(std::process::Stdio::null())
        .output();
    if let Ok(o) = output {
        if o.status.success() {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let pkgs: Vec<String> = stdout.lines()
                .filter(|l| !l.is_empty())
                .map(|s| s.trim().to_string())
                .collect();
            if !pkgs.is_empty() {
                return Ok(pkgs);
            }
        }
    }

    Ok(Vec::new())
}

pub fn undo_transaction(id: i64) -> SpmResult<()> {
    db::with_write_lock(|conn| {
        let tx = db::get_transaction(conn, id)?;
        match tx {
            Some(_) => {
                let files = db::get_files_for_transaction(conn, id)?;
                for f in &files {
                    match f.action {
                        FileAction::Created => {
                            if let Err(e) = fs::remove_file(&f.filepath) {
                                tracing::warn!("Failed to remove {}: {}", f.filepath, e);
                            }
                        }
                        FileAction::Modified => {
                            let backup = paths::backup_path(&f.hash);
                            if backup.exists() {
                                if let Err(e) = fs::copy(&backup, &f.filepath) {
                                    tracing::warn!("Failed to restore backup for {}: {}", f.filepath, e);
                                }
                            }
                        }
                        FileAction::Deleted => {}
                    }
                }
                db::update_transaction_status(conn, id, &TransactionStatus::Undone)?;
                println!("Undone transaction {}", id);
                Ok(())
            }
            None => Err(SpmError::other(format!("Transaction {id} not found"))),
        }
    })
}

pub fn format_history_table(txs: &[Transaction]) -> String {
    if txs.is_empty() {
        return "No transactions found".to_string();
    }
    let mut output = String::new();
    output.push_str(&format!("{:<5} {:<12} {:<20} {:<10} {}\n",
        "ID", "Action", "Date", "Status", "Packages"));
    output.push_str(&"-".repeat(80));
    output.push('\n');
    for tx in txs {
        let date_str = if tx.timestamp.len() >= 19 { &tx.timestamp[..19] } else { &tx.timestamp };
        output.push_str(&format!(
            "{:<5} {:<12} {:<20} {:<10} {}\n",
            tx.id.unwrap_or(0),
            format!("{:?}", tx.action),
            date_str,
            format!("{:?}", tx.status),
            tx.packages.join(", "),
        ));
    }
    output
}

pub fn show_history() -> SpmResult<String> {
    let conn = db::get_connection()?;
    let txs = db::list_transactions(&conn)?;
    Ok(format_history_table(&txs))
}

pub fn create_snapshot() -> SpmResult<()> {
    if !Path::new("/sbin/btrfs").exists() && !Path::new("/usr/sbin/btrfs").exists() {
        return Err(SpmError::other("Btrfs not available on this system"));
    }
    let timestamp = Utc::now().format("%Y-%m-%d_%H%M%S");
    let snap_dir = "/.spm-snapshots";
    fs::create_dir_all(snap_dir)?;
    let snap_name = format!("{}/root-{}", snap_dir, timestamp);
    let output = Command::new("btrfs")
        .args(["subvolume", "snapshot", "-r", "/", &snap_name])
        .output()
        .map_err(|e| SpmError::command_failed(format!("Failed to create Btrfs snapshot: {e}")))?;
    if output.status.success() {
        println!("Btrfs snapshot created: {}", snap_name);
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Invalid argument") || stderr.contains("not supported") {
            return Err(SpmError::other(
                "Btrfs snapshots not supported on this filesystem. Is / mounted with btrfs?"
            ));
        }
        return Err(SpmError::command_failed(format!("Failed to create Btrfs snapshot: {stderr}")));
    }
    Ok(())
}

pub fn rollback_snapshot(id: &str) -> SpmResult<()> {
    if !Path::new("/sbin/btrfs").exists() && !Path::new("/usr/sbin/btrfs").exists() {
        return Err(SpmError::other("Btrfs not available on this system"));
    }
    let output = Command::new("btrfs")
        .args(["subvolume", "snapshot", id, "/"])
        .output()
        .map_err(|e| SpmError::command_failed(format!("Failed to rollback Btrfs snapshot: {e}")))?;
    if output.status.success() {
        println!("Rolled back to snapshot {}", id);
    } else {
        return Err(SpmError::command_failed(format!(
            "Failed to rollback snapshot: {}", String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tx(id: i64, action: &str, ts: &str, status: &str, pkgs: &[&str]) -> Transaction {
        Transaction {
            id: Some(id),
            action: serde_json::from_str(&format!("\"{}\"", action)).unwrap(),
            timestamp: ts.to_string(),
            user: "root".to_string(),
            status: serde_json::from_str(&format!("\"{}\"", status)).unwrap(),
            packages: pkgs.iter().map(|s| s.to_string()).collect(),
            snapshot_id: None,
        }
    }

    // -- format_history_table --

    #[test]
    fn test_format_history_empty() {
        assert_eq!(format_history_table(&[]), "No transactions found");
    }

    #[test]
    fn test_format_history_single() {
        let txs = vec![sample_tx(1, "Install", "2026-01-15T10:30:00Z", "Completed", &["nginx"])];
        let output = format_history_table(&txs);
        assert!(output.contains("1"));
        assert!(output.contains("Install"));
        assert!(output.contains("nginx"));
        assert!(output.contains("ID"));
    }

    #[test]
    fn test_format_history_multiple() {
        let txs = vec![
            sample_tx(1, "Install", "2026-01-15T10:30:00Z", "Completed", &["nginx"]),
            sample_tx(2, "Remove", "2026-01-16T12:00:00Z", "Completed", &["curl"]),
        ];
        let output = format_history_table(&txs);
        assert!(output.contains("1"));
        assert!(output.contains("2"));
        assert!(output.contains("Install"));
        assert!(output.contains("Remove"));
    }

    #[test]
    fn test_format_history_without_id() {
        let mut tx = sample_tx(0, "Install", "2026-01-15T10:30:00Z", "Completed", &["pkg"]);
        tx.id = None;
        let output = format_history_table(&[tx]);
        assert!(output.contains("0"));
    }

    #[test]
    fn test_format_history_truncates_timestamp() {
        let tx = sample_tx(1, "Install", "2026-01-15T10:30:00.123456Z", "Completed", &["pkg"]);
        let output = format_history_table(&[tx]);
        // Should only show "2026-01-15T10:30:00" (first 19 chars)
        assert!(output.contains("2026-01-15T10:30:00"));
    }

    #[test]
    fn test_format_history_short_timestamp() {
        let tx = sample_tx(1, "Install", "short", "Completed", &["pkg"]);
        let output = format_history_table(&[tx]);
        assert!(output.contains("short"));
    }

    // -- match_native_packages --

    fn sample_index(pkgs: &[(&str, &str)]) -> RepoIndex {
        RepoIndex {
            repo_name: "test".into(),
            format_version: 1,
            packages: pkgs.iter().map(|(n, d)| RepoIndexRecord {
                name: n.to_string(),
                version: "1.0".into(),
                architecture: "amd64".into(),
                description: d.to_string(),
                dependencies: vec![],
                provides_soname: vec![],
                conflicts: vec![],
                filename: format!("{}.sam", n),
                hash: "abc".into(),
                size: 1024,
            }).collect(),
        }
    }

    #[test]
    fn test_match_native_packages_name_match() {
        let index = sample_index(&[("nginx", "web server"), ("curl", "http client")]);
        let results = match_native_packages("nginx", &index);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "nginx");
    }

    #[test]
    fn test_match_native_packages_description_match() {
        let index = sample_index(&[("nginx", "web server"), ("curl", "http client")]);
        let results = match_native_packages("web", &index);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "nginx");
    }

    #[test]
    fn test_match_native_packages_case_insensitive() {
        let index = sample_index(&[("NGINX", "Web Server")]);
        let results = match_native_packages("nginx", &index);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_match_native_packages_no_match() {
        let index = sample_index(&[("nginx", "web server")]);
        let results = match_native_packages("mysql", &index);
        assert!(results.is_empty());
    }

    #[test]
    fn test_match_native_packages_multiple_matches() {
        let index = sample_index(&[("nginx", "web server"), ("nginx-light", "web server light")]);
        let results = match_native_packages("nginx", &index);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_match_native_packages_empty_index() {
        let index = sample_index(&[]);
        let results = match_native_packages("nginx", &index);
        assert!(results.is_empty());
    }

    #[test]
    fn test_match_native_packages_format_and_repo() {
        let index = sample_index(&[("nginx", "web")]);
        let results = match_native_packages("nginx", &index);
        assert_eq!(results[0].format, PackageFormat::Sam);
        assert_eq!(results[0].source_repo.as_deref(), Some("native"));
    }
}
