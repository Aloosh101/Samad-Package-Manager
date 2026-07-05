use chrono::Utc;
use std::fs;
use std::path::Path;
use crate::config::{paths, repos};
use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::integration;
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

/// Search cached Packages files for a matching package name.
fn search_deb_cache(repo_name: &str, query: &str) -> Vec<Package> {
    let mut results = Vec::new();
    let q = query.to_lowercase();
    let deb_cache = crate::config::paths::repos_cache_dir().join("deb").join(repo_name);
    if !deb_cache.exists() {
        return results;
    }
    if let Ok(entries) = fs::read_dir(&deb_cache) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name_str) = path.file_name().and_then(|s| s.to_str()) {
                if name_str.starts_with("Packages-") {
                    if let Ok(text) = fs::read_to_string(&path) {
                        let mut current_name = String::new();
                        let mut current_desc = String::new();
                        for line in text.lines() {
                            if line.is_empty() {
                                if !current_name.is_empty() && current_name.to_lowercase().contains(&q) {
                                    results.push(Package {
                                        name: current_name.clone(),
                                        version: String::new(),
                                        format: PackageFormat::Deb,
                                        source_repo: Some("apt".to_string()),
                                        ..Default::default()
                                    });
                                }
                                current_name.clear();
                                current_desc.clear();
                                continue;
                            }
                            if let Some(val) = line.strip_prefix("Package: ") {
                                current_name = val.trim().to_string();
                            } else if let Some(val) = line.strip_prefix("Description: ") {
                                current_desc = val.trim().to_string();
                            }
                        }
                        if !current_name.is_empty() && current_name.to_lowercase().contains(&q) {
                            results.push(Package {
                                name: current_name,
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
    }
    results
}

/// Search cached SONAME index for RPM packages matching query.
fn search_rpm_cache(query: &str) -> Vec<Package> {
    let q = query.to_lowercase();
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Ok(index) = crate::index::SonameIndex::load() {
        for (_key, entry) in &index.entries {
            // Package names are registered in the index as their own key
            for provider in &entry.providers {
                if provider.source == RepoSource::Rpm
                    && !seen.contains(&provider.pkg)
                    && provider.pkg.to_lowercase().contains(&q)
                {
                    seen.insert(provider.pkg.clone());
                    results.push(Package {
                        name: provider.pkg.clone(),
                        version: provider.version.clone(),
                        format: PackageFormat::Rpm,
                        source_repo: Some("dnf".to_string()),
                        ..Default::default()
                    });
                }
            }
        }
    }
    // Also search through cached repo index from spm update
    if let Ok(content) = get_rpm_repo_index() {
        if let Some(index) = content {
            for record in &index.packages {
                if record.name.to_lowercase().contains(&q) && !seen.contains(&record.name) {
                    seen.insert(record.name.clone());
                    results.push(Package {
                        name: record.name.clone(),
                        version: record.version.clone(),
                        format: PackageFormat::Rpm,
                        source_repo: Some("dnf".to_string()),
                        ..Default::default()
                    });
                }
            }
        }
    }
    results
}

fn get_rpm_repo_index() -> SpmResult<Option<crate::types::RepoIndex>> {
    let cache_dir = paths::repos_cache_dir().join("rpm");
    if !cache_dir.exists() {
        return Ok(None);
    }
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let index_path = entry.path().join("repo-index.json");
            if index_path.exists() {
                if let Ok(content) = fs::read_to_string(&index_path) {
                    if let Ok(index) = serde_json::from_str(&content) {
                        return Ok(Some(index));
                    }
                }
            }
        }
    }
    Ok(None)
}

pub fn search_packages(query: &str) -> SpmResult<Vec<Package>> {
    let mut results = Vec::new();
    let repos_list = repos::load_repos()?;
    for (_repo_name, repo_config) in &repos_list {
        match repo_config.source {
            RepoSource::Deb => {
                results.extend(search_deb_cache(_repo_name, query));
            }
            RepoSource::Rpm => {
                results.extend(search_rpm_cache(query));
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
    // Search cached repo data for package info
    let repos_list = repos::load_repos()?;
    for (_repo_name, repo_config) in &repos_list {
        match repo_config.source {
            RepoSource::Deb => {
                let deb_cache = paths::repos_cache_dir().join("deb").join(_repo_name);
                if deb_cache.exists() {
                    if let Ok(entries) = fs::read_dir(&deb_cache) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
                                if fname.starts_with("Packages-") {
                                    if let Ok(text) = fs::read_to_string(&p) {
                                        let mut in_pkg = false;
                                        let mut entry_text = String::new();
                                        for line in text.lines() {
                                            if line.is_empty() {
                                                if in_pkg {
                                                    return Ok(entry_text);
                                                }
                                                entry_text.clear();
                                                in_pkg = false;
                                                continue;
                                            }
                                            if let Some(val) = line.strip_prefix("Package: ") {
                                                if val.trim().eq_ignore_ascii_case(name) {
                                                    in_pkg = true;
                                                }
                                            }
                                            if in_pkg {
                                                entry_text.push_str(line);
                                                entry_text.push('\n');
                                            }
                                        }
                                        if in_pkg && !entry_text.is_empty() {
                                            return Ok(entry_text);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            RepoSource::Rpm => {
                // Look up in SONAME index or cached RPM repo data
                if let Ok(index) = crate::index::SonameIndex::load() {
                    if let Some(providers) = index.get_providers(name) {
                        if let Some(provider) = providers.first() {
                            if provider.source == RepoSource::Rpm {
                                return Ok(format!(
                                    "Package: {}\nVersion: {}\nRepository: {}\n",
                                    provider.pkg, provider.version, provider.repo
                                ));
                            }
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
    // Fallback: read native DB files
    if let Ok(content) = std::fs::read_to_string(format!("/var/lib/dpkg/info/{name}.list")) {
        return Ok(content.lines().map(|s| s.to_string()).filter(|l| !l.is_empty()).collect());
    }
    // Try RPM sqlite DB
    if let Ok(conn) = rusqlite::Connection::open("/var/lib/rpm/rpmdb.sqlite") {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT fi.name FROM files fi JOIN packages p ON p.packageId = fi.packageId WHERE p.name = ?1"
        ) {
            if let Ok(rows) = stmt.query_map([name], |row| row.get::<_, String>(0)) {
                let files: Vec<_> = rows.filter_map(|r| r.ok()).collect();
                if !files.is_empty() {
                    return Ok(files);
                }
            }
        }
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
    // Fallback: read from SONAME index / cached repo data
    if let Ok(index) = crate::index::SonameIndex::load() {
        if let Some(requires) = index.get_requires(name) {
            if !requires.is_empty() {
                return Ok(requires.iter().map(|r| Dependency {
                    name: r.to_string(),
                    version: String::new(),
                    source: DependencySource::System,
                    format: None,
                }).collect());
            }
        }
    }
    Ok(Vec::new())
}

pub fn reverse_dependencies(name: &str) -> SpmResult<Vec<String>> {
    // Scan SPM DB: from installed packages' manifests
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
    Ok(Vec::new())
}

/// Find which installed package owns a given file path.
/// Queries the SPM DB first.
pub fn search_file_owner(path: &str) -> SpmResult<Vec<String>> {
    let resolved = Path::new(path);
    let path_str = resolved.to_string_lossy().to_string();

    // 1. Search SPM DB files table
    if let Ok(conn) = db::get_connection() {
        if let Ok(files) = db::get_packages_for_file(&conn, &path_str) {
            if !files.is_empty() {
                return Ok(files);
            }
        }
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

    // 2. Try native DB files as fallback
    if let Ok(content) = std::fs::read_to_string(format!("/var/lib/dpkg/info/{path}.list")) {
        return Ok(vec![content]);
    }
    // Scan dpkg info directory for matching list files
    if let Ok(entries) = std::fs::read_dir("/var/lib/dpkg/info") {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("list") {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if content.lines().any(|l| l.trim() == path_str) {
                        if let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().to_string()) {
                            return Ok(vec![stem]);
                        }
                    }
                }
            }
        }
    }

    // 3. Try RPM sqlite DB
    if let Ok(conn) = rusqlite::Connection::open("/var/lib/rpm/rpmdb.sqlite") {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT DISTINCT p.name FROM packages p \
             JOIN files fi ON fi.packageId = p.packageId \
             WHERE fi.name = ?1 LIMIT 10"
        ) {
            if let Ok(rows) = stmt.query_map([path_str], |row| row.get::<_, String>(0)) {
                let pkgs: Vec<_> = rows.filter_map(|r| r.ok()).collect();
                if !pkgs.is_empty() {
                    return Ok(pkgs);
                }
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
    if !integration::fs::is_btrfs_subvolume(Path::new("/"))? {
        return Err(SpmError::other("Btrfs not available on this system"));
    }
    let timestamp = Utc::now().format("%Y-%m-%d_%H%M%S");
    let snap_dir = "/.spm-snapshots";
    fs::create_dir_all(snap_dir)?;
    let snap_name = format!("root-{}", timestamp);
    integration::fs::create_btrfs_snapshot(Path::new("/"), Path::new(snap_dir), &snap_name)?;
    println!("Btrfs snapshot created: {}/{}", snap_dir, snap_name);
    Ok(())
}

pub fn rollback_snapshot(id: &str) -> SpmResult<()> {
    if !integration::fs::is_btrfs_subvolume(Path::new("/"))? {
        return Err(SpmError::other("Btrfs not available on this system"));
    }
    integration::fs::rollback_btrfs_snapshot(Path::new(id), Path::new("/"))?;
    println!("Rolled back to snapshot {}", id);
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
