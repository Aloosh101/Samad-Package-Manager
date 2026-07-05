use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use chrono::Utc;
use tokio::task::JoinSet;

use crate::db;
use crate::db::conflict;
use crate::error::{SpmError, SpmResult};
use crate::package::transaction::TransactionEngine;
use crate::package::{fetch, hooks, scripts, store as pkg_store};
use crate::store::StoreManager;
use crate::types::*;
use crate::util::hash;

use super::permission::PeerCreds;

#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub packages: Vec<PackageId>,
    pub action: TransactionAction,
    pub flags: HashMap<String, String>,
}

impl InstallPlan {
    pub fn new(packages: Vec<PackageId>, action: TransactionAction) -> Self {
        Self {
            packages,
            action,
            flags: HashMap::new(),
        }
    }

    pub fn package_names(&self) -> Vec<String> {
        self.packages.iter().map(|p| p.name.clone()).collect()
    }

    pub fn flag(&self, key: &str) -> Option<&String> {
        self.flags.get(key)
    }
}

#[derive(Debug, Clone)]
pub struct TransactionResult {
    pub transaction_id: i64,
    pub packages: Vec<String>,
    pub action: TransactionAction,
    pub status: TransactionStatus,
    pub timestamp: String,
}

pub struct TransactionManager {
    store: StoreManager,
    hooks: bool,
    #[allow(dead_code)]
    sandbox_launcher: Option<String>,
}

impl TransactionManager {
    pub fn new(store: StoreManager, hooks: bool, sandbox_launcher: Option<String>) -> Self {
        Self {
            store,
            hooks,
            sandbox_launcher,
        }
    }

    pub fn store(&self) -> &StoreManager {
        &self.store
    }

    pub async fn execute_install(
        &self,
        plan: InstallPlan,
        creds: &PeerCreds,
    ) -> SpmResult<TransactionResult> {
        // ═══════════════════════════════════════════════════════════
        // Phase 1: Fetch & Verify (parallel)
        // ═══════════════════════════════════════════════════════════
        let mut fetched = self.phase_fetch_verify(&plan, creds).await?;

        // ═══════════════════════════════════════════════════════════
        // Phase 2: Conflict Detection
        // ═══════════════════════════════════════════════════════════
        let (removed, file_conflicts) = self.phase_conflict_detection(&plan, &fetched)?;

        // ═══════════════════════════════════════════════════════════
        // Phase 3: Prepare Store (copy + deduplicate)
        // ═══════════════════════════════════════════════════════════
        let (_store_hashes, mut guards) = self.phase_prepare_store(&mut fetched, &removed)?;

        // ═══════════════════════════════════════════════════════════
        // Phase 4: Run Pre-install Scripts
        // ═══════════════════════════════════════════════════════════
        self.phase_preinstall_scripts(&fetched)?;

        // ═══════════════════════════════════════════════════════════
        // Phase 5: Atomic Filesystem Deployment
        // ═══════════════════════════════════════════════════════════
        let guard_count = guards.len();
        let db_result = db::with_write_lock(|conn| {
            let engine = TransactionEngine::new(conn);
            self.phase_atomic_deploy(&engine, &plan, &mut fetched, &removed, &file_conflicts)
        });

        match db_result {
            Ok(tx_id) => {
                for g in &mut guards {
                    g.disarm();
                }

                // ═══════════════════════════════════════════════════
                // Phase 6: Post-install (scripts + hooks)
                // ═══════════════════════════════════════════════════
                self.phase_post_install(&fetched, &removed)?;

                let package_names: Vec<String> =
                    plan.packages.iter().map(|p| p.name.clone()).collect();
                Ok(TransactionResult {
                    transaction_id: tx_id,
                    packages: package_names,
                    action: plan.action,
                    status: TransactionStatus::Completed,
                    timestamp: Utc::now().to_rfc3339(),
                })
            }
            Err(e) => {
                tracing::error!("Transaction failed, rolling back {} guards", guard_count);
                Err(e)
            }
        }
    }

    // ═══════════════════════════════════════════════════════════
    // Phase 1: Fetch & Verify
    // ═══════════════════════════════════════════════════════════

    async fn phase_fetch_verify(
        &self,
        plan: &InstallPlan,
        _creds: &PeerCreds,
    ) -> SpmResult<Vec<fetch::FetchedPackage>> {
        let repos = crate::config::repos::load_repos()?;
        let mut set = JoinSet::new();

        for pid in &plan.packages {
            let name = pid.name.clone();
            let format = pid.format.clone();
            let repos = repos.clone();
            set.spawn(async move {
                let matching_source = match format {
                    PackageFormat::Deb => RepoSource::Deb,
                    PackageFormat::Rpm => RepoSource::Rpm,
                    PackageFormat::Sam => RepoSource::Native,
                };
                for (rn, rc) in &repos {
                    if rc.source == matching_source {
                        let conn = match crate::db::get_connection() {
                            Ok(c) => c,
                            Err(_) => return None,
                        };
                        if let Ok(fetched) = fetch::fetch_and_extract(&name, rn, rc, false, &conn)
                        {
                            return Some(fetched);
                        }
                    }
                }
                for (rn, rc) in &repos {
                    if rc.source == RepoSource::Native && matching_source != RepoSource::Native {
                        let conn = match crate::db::get_connection() {
                            Ok(c) => c,
                            Err(_) => return None,
                        };
                        if let Ok(fetched) = fetch::fetch_and_extract(&name, rn, rc, false, &conn)
                        {
                            return Some(fetched);
                        }
                    }
                }
                None
            });
        }

        let mut results = Vec::new();
        while let Some(res) = set.join_next().await {
            match res {
                Ok(Some(fetched)) => results.push(fetched),
                Ok(None) => {
                    return Err(SpmError::package_not_found(
                        "One or more packages could not be fetched from any repository",
                    ));
                }
                Err(e) => {
                    return Err(SpmError::other(format!("Fetch task failed: {e}")));
                }
            }
        }

        results.sort_by(|a: &fetch::FetchedPackage, b: &fetch::FetchedPackage| {
            let a_idx = plan
                .packages
                .iter()
                .position(|p| p.name == a.pkg.name)
                .unwrap_or(usize::MAX);
            let b_idx = plan
                .packages
                .iter()
                .position(|p| p.name == b.pkg.name)
                .unwrap_or(usize::MAX);
            a_idx.cmp(&b_idx)
        });

        Ok(results)
    }

    // ═══════════════════════════════════════════════════════════
    // Phase 2: Conflict Detection
    // ═══════════════════════════════════════════════════════════

    fn phase_conflict_detection(
        &self,
        plan: &InstallPlan,
        fetched: &[fetch::FetchedPackage],
    ) -> SpmResult<(Vec<String>, HashMap<String, Vec<String>>)> {
        let mut removed: Vec<String> = Vec::new();
        let mut all_file_conflicts: HashMap<String, Vec<String>> = HashMap::new();

        if let Some(replace_pkg) = plan.flag("replace") {
            if !removed.contains(replace_pkg) {
                removed.push(replace_pkg.clone());
            }
        }

        for f in fetched {
            for obsolete in &f.manifest.obsoletes {
                if db::with_read_lock(|conn| Ok(db::is_installed(conn, obsolete)))?
                    && !removed.contains(obsolete)
                {
                    removed.push(obsolete.clone());
                }
            }
        }

        let new_files: std::collections::HashSet<String> = fetched
            .iter()
            .flat_map(|f| f.files.iter())
            .filter(|fr| matches!(fr.action, FileAction::Created))
            .map(|fr| fr.filepath.clone())
            .collect();

        if !new_files.is_empty() {
            let file_conflicts = db::with_read_lock(|conn| {
                conflict::detect_file_conflicts(conn, &new_files, &removed)
            })?;

            for (pkg, files) in &file_conflicts {
                if !removed.contains(pkg) {
                    if plan.flag("replace").is_some() {
                        removed.push(pkg.clone());
                        all_file_conflicts
                            .entry(pkg.clone())
                            .or_default()
                            .extend(files.clone());
                    } else {
                        return Err(SpmError::other(format!(
                            "File conflicts detected with '{}'. Use --replace to force install.",
                            pkg,
                        )));
                    }
                }
            }
        }

        Ok((removed, all_file_conflicts))
    }

    // ═══════════════════════════════════════════════════════════
    // Phase 3: Prepare Store
    // ═══════════════════════════════════════════════════════════

    fn phase_prepare_store(
        &self,
        fetched: &mut [fetch::FetchedPackage],
        removed: &[String],
    ) -> SpmResult<(Vec<String>, Vec<RollbackGuard>)> {
        let dep_hashes: Vec<String> = db::with_read_lock(|conn| {
            let installed = db::list_installed_packages(conn)?;
            let installed_names: std::collections::HashSet<String> =
                installed.into_iter().map(|p| p.name).collect();
            let mut hashes = Vec::new();
            for f in fetched.iter() {
                if installed_names.contains(&f.pkg.name) && !removed.contains(&f.pkg.name) {
                    if let Ok(Some(h)) = db::get_store_hash(conn, &f.pkg.name, &f.pkg.format) {
                        hashes.push(h);
                    }
                }
            }
            Ok(hashes)
        })?;

        let mut store_hashes = Vec::new();
        let mut guards = Vec::new();

        for f in fetched.iter_mut() {
            let origin = pkg_store::origin_from_format(&f.pkg.format);
            let data_dir = Path::new(&f.extracted_dir);

            if data_dir.exists() {
                let pkg_hash = hash::hash_dir(data_dir)?;
                let store_path = pkg_store::copy_to_store_with_origin(data_dir, &pkg_hash, origin)?;

                let dep_store_dirs: Vec<PathBuf> = dep_hashes
                    .iter()
                    .map(|h| pkg_store::store_package_dir_for_origin(h, origin))
                    .filter(|d| d.exists())
                    .collect();

                pkg_store::set_rpath_on_elfs(&store_path, &dep_store_dirs)?;

                let symlinks = pkg_store::create_fhs_symlinks(&store_path)?;

                let mut guard = RollbackGuard::new();
                guard.set_store_hash(pkg_hash.clone());
                guard.set_store_origin(origin.to_string());
                guard.set_symlinks(symlinks);
                guards.push(guard);

                f.pkg.store_hash = Some(pkg_hash.clone());
                store_hashes.push(pkg_hash);
            }

            if !f.scripts.is_empty() {
                scripts::save_scripts(&f.pkg.name, &f.scripts)?;
            }
        }

        Ok((store_hashes, guards))
    }

    // ═══════════════════════════════════════════════════════════
    // Phase 4: Pre-install Scripts
    // ═══════════════════════════════════════════════════════════

    fn phase_preinstall_scripts(&self, fetched: &[fetch::FetchedPackage]) -> SpmResult<()> {
        for f in fetched {
            if let Some(ref script) = f.scripts.preinst {
                tracing::info!("Running preinst for {}", f.pkg.name);
                let _ = scripts::run_script(script, "install");
            }
        }
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════
    // Phase 5: Atomic DB Commit (wrapped in write lock)
    // ═══════════════════════════════════════════════════════════

    fn phase_atomic_deploy(
        &self,
        _engine: &TransactionEngine,
        plan: &InstallPlan,
        fetched: &mut [fetch::FetchedPackage],
        removed: &[String],
        file_conflicts: &HashMap<String, Vec<String>>,
    ) -> SpmResult<i64> {
        let conn = db::get_connection()?;
        conn.execute_batch("BEGIN")?;

        let result = (|| -> SpmResult<i64> {
            let all_pkg_names: Vec<String> = plan.packages.iter().map(|p| p.name.clone()).collect();
            let tx_id = db::record_transaction(
                &conn,
                &Transaction {
                    id: None,
                    action: TransactionAction::Install,
                    timestamp: Utc::now().to_rfc3339(),
                    user: "spmd".to_string(),
                    status: TransactionStatus::Completed,
                    packages: all_pkg_names,
                    snapshot_id: None,
                },
            )?;

            for name in removed {
                if let Some(pkg) = db::get_installed_package(&conn, name)? {
                    let pid = PackageId::new(name, pkg.format);
                    db::remove_installed_package_by_id(&conn, &pid)?;
                }
            }

            for f in fetched.iter() {
                let files_with_tx: Vec<FileRecord> = f
                    .files
                    .iter()
                    .map(|fr| {
                        let mut f2 = fr.clone();
                        f2.transaction_id = tx_id;
                        f2
                    })
                    .collect();

                if !files_with_tx.is_empty() {
                    db::record_files(&conn, &files_with_tx)?;
                }
                db::add_installed_package(&conn, &f.pkg)?;
            }

            if !file_conflicts.is_empty() {
                conflict::record_conflicts_batch(
                    &conn,
                    tx_id,
                    file_conflicts,
                    &plan.packages[0].name,
                )?;
            }

            Ok(tx_id)
        })();

        match result {
            Ok(tx_id) => {
                conn.execute_batch("COMMIT")?;
                Ok(tx_id)
            }
            Err(e) => {
                conn.execute_batch("ROLLBACK")?;
                Err(e)
            }
        }
    }

    // ═══════════════════════════════════════════════════════════
    // Phase 6: Post-install (scripts + hooks)
    // ═══════════════════════════════════════════════════════════

    fn phase_post_install(
        &self,
        fetched: &[fetch::FetchedPackage],
        removed: &[String],
    ) -> SpmResult<()> {
        for f in fetched {
            if let Some(ref script) = f.scripts.postinst {
                tracing::info!("Running postinst for {}", f.pkg.name);
                let _ = scripts::run_script(script, "configure");
            }
        }

        if self.hooks {
            let all_files: Vec<String> = fetched
                .iter()
                .flat_map(|f| f.files.iter().map(|fr| fr.filepath.clone()))
                .collect();

            hooks::run_install_hooks(&all_files);

            let installed_names: Vec<String> = fetched.iter().map(|f| f.pkg.name.clone()).collect();

            hooks::run_kernel_hooks(&installed_names);

            let manifests: Vec<&Manifest> = fetched.iter().map(|f| &f.manifest).collect();

            hooks::run_sam_v2_hooks(&manifests);
            crate::package::triggers::run_triggers("install", &installed_names);
            if !removed.is_empty() {
                crate::package::triggers::run_triggers("remove", removed);
            }
        }

        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════
// RollbackGuard
// ═══════════════════════════════════════════════════════════════

pub struct RollbackGuard {
    store_hash: Option<String>,
    store_origin: String,
    symlinks: Vec<pkg_store::SymlinkRecord>,
    armed: bool,
}

impl RollbackGuard {
    pub fn new() -> Self {
        Self {
            store_hash: None,
            store_origin: String::new(),
            symlinks: Vec::new(),
            armed: true,
        }
    }

    pub fn set_store_hash(&mut self, hash: String) {
        self.store_hash = Some(hash);
    }

    pub fn set_store_origin(&mut self, origin: String) {
        self.store_origin = origin;
    }

    pub fn set_symlinks(&mut self, symlinks: Vec<pkg_store::SymlinkRecord>) {
        self.symlinks = symlinks;
    }

    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RollbackGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for s in &self.symlinks {
            let _ = fs::remove_file(&s.fhs_path);
            if let Some(parent) = s.fhs_path.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
        if let Some(ref hash) = self.store_hash {
            let store_dir = if self.store_origin.is_empty() {
                pkg_store::store_package_dir(hash)
            } else {
                pkg_store::store_package_dir_for_origin(hash, &self.store_origin)
            };
            if store_dir.exists() {
                let _ = fs::remove_dir_all(&store_dir);
            }
        }
    }
}

unsafe impl Send for RollbackGuard {}
unsafe impl Sync for RollbackGuard {}

/// Represents the 7 phases of the daemon execution flow
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    FetchVerify,
    ConflictDetection,
    PrepareStore,
    PreInstallScripts,
    AtomicDeploy,
    PostInstall,
    Cleanup,
}

/// Runs pre/post install hook scripts with a configurable timeout
pub struct HookRunner {
    timeout: Duration,
}

impl HookRunner {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    pub fn with_default_timeout() -> Self {
        Self {
            timeout: Duration::from_secs(300),
        }
    }

    pub fn run_preinstall(&self, package: &str, script: &str) -> SpmResult<()> {
        tracing::info!("Running preinst hook for {package}");
        let result = self.run_script(script, &["install"]);
        match result {
            Ok(_) => tracing::info!("preinst hook for {package} completed"),
            Err(ref e) => tracing::warn!("preinst hook for {package} failed: {e}"),
        }
        result
    }

    pub fn run_postinstall(&self, package: &str, script: &str) -> SpmResult<()> {
        tracing::info!("Running postinst hook for {package}");
        let result = self.run_script(script, &["configure"]);
        match result {
            Ok(_) => tracing::info!("postinst hook for {package} completed"),
            Err(ref e) => tracing::warn!("postinst hook for {package} failed: {e}"),
        }
        result
    }

    pub fn run_preremove(&self, package: &str, script: &str) -> SpmResult<()> {
        tracing::info!("Running prerm hook for {package}");
        self.run_script(script, &["remove"])
    }

    pub fn run_postremove(&self, package: &str, script: &str) -> SpmResult<()> {
        tracing::info!("Running postrm hook for {package}");
        self.run_script(script, &["purge"])
    }

    fn run_script(&self, script: &str, args: &[&str]) -> SpmResult<()> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut child = Command::new(&shell)
            .arg("-e")
            .arg(script)
            .args(args)
            .spawn()
            .map_err(|e| SpmError::other(format!("Failed to spawn hook script: {e}")))?;

        let now = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        return Ok(());
                    } else {
                        return Err(SpmError::command_failed(format!(
                            "Hook script exited with status: {status}"
                        )));
                    }
                }
                Ok(None) => {
                    if now.elapsed() > self.timeout {
                        let _ = child.kill();
                        return Err(SpmError::command_failed(
                            "Hook script timed out",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(SpmError::other(format!(
                        "Failed to wait on hook script: {e}"
                    )));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_plan_new() {
        let packages = vec![PackageId::new("nginx", PackageFormat::Sam)];
        let plan = InstallPlan::new(packages.clone(), TransactionAction::Install);
        assert_eq!(plan.packages.len(), 1);
        assert_eq!(plan.packages[0].name, "nginx");
        assert!(matches!(plan.action, TransactionAction::Install));
    }

    #[test]
    fn test_install_plan_package_names() {
        let packages = vec![
            PackageId::new("nginx", PackageFormat::Sam),
            PackageId::new("libpcre3", PackageFormat::Deb),
        ];
        let plan = InstallPlan::new(packages, TransactionAction::Install);
        let names = plan.package_names();
        assert_eq!(names, vec!["nginx", "libpcre3"]);
    }

    #[test]
    fn test_install_plan_flags() {
        let mut plan = InstallPlan::new(vec![], TransactionAction::Install);
        plan.flags.insert("replace".to_string(), "true".to_string());
        assert_eq!(plan.flag("replace"), Some(&"true".to_string()));
        assert_eq!(plan.flag("nonexistent"), None);
    }

    #[test]
    fn test_transaction_result_fields() {
        let result = TransactionResult {
            transaction_id: 42,
            packages: vec!["nginx".to_string()],
            action: TransactionAction::Install,
            status: TransactionStatus::Completed,
            timestamp: "2026-07-04T12:00:00Z".to_string(),
        };
        assert_eq!(result.transaction_id, 42);
        assert_eq!(result.packages, vec!["nginx"]);
        assert!(matches!(result.status, TransactionStatus::Completed));
        assert!(matches!(result.action, TransactionAction::Install));
    }

    #[test]
    fn test_rollback_guard_new() {
        let guard = RollbackGuard::new();
        assert!(guard.armed);
        assert!(guard.store_hash.is_none());
        assert!(guard.symlinks.is_empty());
    }

    #[test]
    fn test_rollback_guard_disarm() {
        let mut guard = RollbackGuard::new();
        guard.disarm();
        assert!(!guard.armed);
    }

    #[test]
    fn test_rollback_guard_set_fields() {
        let mut guard = RollbackGuard::new();
        guard.set_store_hash("abc123".to_string());
        guard.set_store_origin("deb".to_string());
        assert_eq!(guard.store_hash, Some("abc123".to_string()));
        assert_eq!(guard.store_origin, "deb");
    }

    #[test]
    fn test_rollback_guard_drop_noop() {
        let guard = RollbackGuard {
            store_hash: None,
            store_origin: String::new(),
            symlinks: Vec::new(),
            armed: true,
        };
        drop(guard);
    }

    #[test]
    fn test_rollback_guard_drop_disarmed() {
        let mut guard = RollbackGuard {
            store_hash: Some("test".to_string()),
            store_origin: "sam".to_string(),
            symlinks: Vec::new(),
            armed: true,
        };
        guard.disarm();
        drop(guard);
    }
}
