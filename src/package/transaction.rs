use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::io::{IsTerminal, Write};
use chrono::Utc;

use crate::config::repos;
use crate::db;
use crate::db::conflict;
use crate::error::{SpmError, SpmResult};
use crate::package::{fetch, scripts, store};
use crate::types::*;
use crate::util::hash;

pub(crate) struct TransactionPlan {
    pub(crate) name: String,
    pub(crate) all_packages: Vec<PackageId>,
    pub(crate) to_remove: Vec<String>,
    pub(crate) file_conflicts: HashMap<String, Vec<String>>,
    pub(crate) classified: (Vec<ConflictSummary>, Vec<ConflictSummary>, Vec<ConflictSummary>),
}

pub(crate) struct TransactionEngine<'a> {
    conn: &'a rusqlite::Connection,
}

impl<'a> TransactionEngine<'a> {
    pub(crate) fn new(conn: &'a rusqlite::Connection) -> Self {
        Self { conn }
    }

    // ═══════════════════════════════════════════
    // Phase 0: Plan
    // ═══════════════════════════════════════════

    pub(crate) fn plan_install_local(
        &self,
        name: &str,
        replace: bool,
        new_files: &[String],
    ) -> SpmResult<TransactionPlan> {
        let mut to_remove: Vec<String> = Vec::new();
        if replace && db::is_installed(self.conn, name) {
            to_remove.push(name.to_string());
        }

        let file_set: HashSet<String> = new_files.iter().cloned().collect();
        let file_conflicts = if !file_set.is_empty() {
            conflict::detect_file_conflicts(self.conn, &file_set, &to_remove)?
        } else {
            HashMap::new()
        };
        let classified = conflict::classify_conflicts(&file_conflicts);

        Ok(TransactionPlan {
            name: name.to_string(),
            all_packages: vec![PackageId::new(name, PackageFormat::Sam)],
            to_remove,
            file_conflicts,
            classified,
        })
    }

    pub(crate) fn plan_install(
        &self,
        name: &str,
        resolved: &crate::package::resolver::ResolvedGraph,
    ) -> SpmResult<TransactionPlan> {
        let mut to_remove: Vec<String> = Vec::new();

        let installed_names = db::get_all_installed_package_names(self.conn)?;
        for installed in &installed_names {
            if installed == name {
                to_remove.push(installed.clone());
            }
        }

        // Detect file conflicts for packages being replaced:
        // files owned by to_remove packages may overlap with other installed packages' files.
        let file_conflicts = if !to_remove.is_empty() {
            let old_files: Vec<String> = db::get_files_by_packages_batch(self.conn, &to_remove)?
                .into_iter()
                .map(|f| f.filepath)
                .collect();
            let file_set: std::collections::HashSet<String> = old_files.into_iter().collect();
            if !file_set.is_empty() {
                conflict::detect_file_conflicts(self.conn, &file_set, &to_remove)?
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };
        let classified = conflict::classify_conflicts(&file_conflicts);

        let all_packages: Vec<PackageId> = resolved.topological_order.clone();

        Ok(TransactionPlan {
            name: name.to_string(),
            all_packages,
            to_remove,
            file_conflicts,
            classified,
        })
    }

    // ═══════════════════════════════════════════
    // Phase 1: Display + Approve
    // ═══════════════════════════════════════════

    pub(crate) fn display_plan(plan: &TransactionPlan) {
        Self::display_plan_smart(plan, false)
    }

    pub(crate) fn display_plan_smart(plan: &TransactionPlan, smart: bool) {
        use crate::output;

        eprint!("\n  {} ", output::bold("📋 Transaction Plan:"));
        eprintln!("{}", output::dim(format!("({} total packages)", plan.all_packages.len())));

        if !plan.to_remove.is_empty() {
            let (critical, shared, minor) = &plan.classified;
            if !critical.is_empty() || !shared.is_empty() || !minor.is_empty() {
                eprintln!("\n    {} Will be REMOVED:", output::red("✖"));

                if !critical.is_empty() {
                    eprintln!("      {} {} packages (bin/lib conflicts):", output::red("▸"), critical.len());
                    for c in critical.iter().take(10) {
                        eprintln!("        {} {} — {}",
                            output::bold("−"),
                            output::bold(&c.package),
                            output::yellow(&c.reason),
                        );
                    }
                    if critical.len() > 10 {
                        eprintln!("        {} ... and {} more", output::dim("└"), critical.len() - 10);
                    }
                }

                if !shared.is_empty() {
                    eprintln!("      {} {} packages (shared files):", output::yellow("▸"), shared.len());
                    for c in shared.iter().take(5) {
                        eprintln!("        {} {} — {}",
                            output::dim("−"),
                            output::bold(&c.package),
                            c.reason,
                        );
                    }
                    if shared.len() > 5 {
                        eprintln!("        {} ... and {} more", output::dim("└"), shared.len() - 5);
                    }
                }

                if !minor.is_empty() {
                    eprintln!("      {} {} packages (minor, auto-resolvable)", output::green("▸"), minor.len());
                }
            } else {
                for pkg in &plan.to_remove {
                    eprintln!("      {} {}", output::red("−"), output::bold(pkg));
                }
            }
        }

        eprintln!("\n    {} Will be INSTALLED:", output::green("✔"));
        for pkg in &plan.all_packages {
            eprintln!("      {} {} ({:?})", output::green("+"), output::bold(&pkg.name), pkg.format);
        }

        let net = plan.all_packages.len() as isize - plan.to_remove.len() as isize;
        eprintln!("\n    {} Net change: {} packages",
            output::cyan("≈"),
            if net >= 0 { format!("+{}", net) } else { format!("{}", net) },
        );

        let total_overlaps: usize = plan.file_conflicts.values().map(|v| v.len()).sum();
        if total_overlaps > 0 {
            eprintln!("    {} Total conflicting files: {}", output::cyan("≈"), total_overlaps);
        }

        if smart {
            eprintln!("    {} Library isolation active — .so files kept in store, RPATH auto-configured", output::green("🧠"));
        }
    }

    pub(crate) fn approve_plan(_plan: &TransactionPlan, yes: bool) -> SpmResult<bool> {
        if yes || !std::io::stdout().is_terminal() {
            return Ok(true);
        }
        print!("  {} Approve this plan? [Y/n]: ", crate::output::green("?"));
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        let input = input.trim().to_lowercase();
        Ok(input.is_empty() || input == "y" || input == "yes")
    }

    // ═══════════════════════════════════════════
    // Phase 2: Execute (4-phase atomic)
    // ═══════════════════════════════════════════

    pub(crate) fn install_local(
        &self,
        fetched: &mut fetch::FetchedPackage,
        replace: bool,
    ) -> SpmResult<()> {
        let name = fetched.pkg.name.clone();

        if db::is_installed(self.conn, &name) && !replace {
            return Err(SpmError::package_already_installed(&name));
        }

        let mut removed: Vec<String> = Vec::new();
        if replace && db::is_installed(self.conn, &name) {
            removed.push(name.clone());
        }

        // Store trigger scripts for SAM v2 triggers
        if !fetched.manifest.triggers.is_empty() {
            let _ = crate::package::triggers::store_triggers(
                &name, &fetched.manifest, &fetched.extracted_dir,
            );
        }

        // Process obsoletes from SAM v2 manifest
        for obsolete_pkg in &fetched.manifest.obsoletes {
            if db::is_installed(self.conn, obsolete_pkg) && !removed.contains(obsolete_pkg) {
                crate::output::step_info(format!(
                    "Package '{}' obsoletes '{}' — will be removed",
                    name, obsolete_pkg
                ));
                removed.push(obsolete_pkg.clone());
            }
        }

        let dep_hashes: Vec<String> = Vec::new();
        let mut guard = self.fs_deploy_one(fetched, &dep_hashes, false)?;
        let remove_infos = self.capture_remove_infos(&removed);

        // Phase C: Atomic DB commit
        self.conn.execute_batch("BEGIN")?;

        let result = (|| -> SpmResult<()> {
            for name in &removed {
                if let Some(pkg) = db::get_installed_package(self.conn, name)? {
                    let pid = PackageId::new(name, pkg.format);
                    db::remove_installed_package_by_id(self.conn, &pid)?;
                }
            }

            let tx_id = db::record_transaction(self.conn, &Transaction {
                id: None,
                action: TransactionAction::Install,
                timestamp: Utc::now().to_rfc3339(),
                user: crate::util::fs::whoami(),
                status: TransactionStatus::Completed,
                packages: vec![name.clone()],
                snapshot_id: None,
            })?;

            let files_with_tx: Vec<FileRecord> = fetched.files.iter().map(|f| {
                let mut f2 = f.clone();
                f2.transaction_id = tx_id;
                f2
            }).collect();

            if !files_with_tx.is_empty() {
                db::record_files(self.conn, &files_with_tx)?;
            }
            db::add_installed_package(self.conn, &fetched.pkg)?;

            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                guard.disarm();

                self.run_package_scripts(fetched);

                for (name, files, store_hash, install_type, manifest, pkg_format) in &remove_infos {
                    let _ = self.cleanup_removed(name, files, store_hash.as_deref(), install_type, manifest.as_ref(), pkg_format);
                }

                let installed_names = vec![fetched.pkg.name.clone()];
                crate::package::hooks::run_kernel_hooks(&installed_names);

                let manifests = vec![&fetched.manifest];
                crate::package::hooks::run_sam_v2_hooks(&manifests);

                crate::package::triggers::run_triggers("install", &installed_names);
                if !removed.is_empty() {
                    crate::package::triggers::run_triggers("remove", &removed);
                }

                crate::output::step_info("Installation complete");
                Ok(())
            }
            Err(e) => {
                self.conn.execute_batch("ROLLBACK")?;
                Err(e)
            }
        }
    }

    pub(crate) fn upgrade_package(&self, name: &str, dep_store_hashes: &[String]) -> SpmResult<()> {
        let repos_list = repos::load_repos()?;
        let old_pkg = db::get_installed_package(self.conn, name)?
            .ok_or_else(|| SpmError::package_not_found(format!("Package '{name}' is not installed")))?;

        let old_files = db::get_files_by_package(self.conn, name)?;
        let old_install_type = old_pkg.install_type.clone();
        let old_store_hash = old_pkg.store_hash.clone();

        // Iterate all repos to find the package (format is now always Sam)
        for (rn, rc) in &repos_list {
            if let Ok(mut fetched) = fetch::fetch_and_extract(name, rn, rc, true, self.conn) {
                // Phase B: Backup conffiles before deploying new version
                backup_conffiles_for_upgrade(&old_files, &fetched.manifest, &fetched.extracted_dir);

                // Phase B: FS deploy new version
                let mut guard = self.fs_deploy_one(&mut fetched, dep_store_hashes, false)?;

                // Phase C: Atomic DB commit (remove old + add new)
                self.conn.execute_batch("BEGIN")?;
                db::remove_installed_package_by_id(self.conn, &PackageId::new(name, old_pkg.format.clone()))?;
                let tx_id = db::record_transaction(self.conn, &Transaction {
                    id: None,
                    action: TransactionAction::Install,
                    timestamp: Utc::now().to_rfc3339(),
                    user: crate::util::fs::whoami(),
                    status: TransactionStatus::Completed,
                    packages: vec![name.to_string()],
                    snapshot_id: None,
                })?;
                let files_with_tx: Vec<FileRecord> = fetched.files.iter().map(|f| {
                    let mut f2 = f.clone();
                    f2.transaction_id = tx_id;
                    f2
                }).collect();
                if !files_with_tx.is_empty() {
                    db::record_files(self.conn, &files_with_tx)?;
                }
                db::add_installed_package(self.conn, &fetched.pkg)?;
                self.conn.execute_batch("COMMIT")?;

                guard.disarm();

                // Phase D: Scripts + FS cleanup old version + SAM v2 hooks
                self.run_package_scripts(&fetched);
                let old_manifest = old_pkg.manifest.as_ref().and_then(|m| serde_json::from_str(m).ok());
                let _ = self.cleanup_removed(name, &old_files, old_store_hash.as_deref(), &old_install_type, old_manifest.as_ref(), &old_pkg.format);

                let upgrade_manifests = vec![&fetched.manifest];
                crate::package::hooks::run_sam_v2_hooks(&upgrade_manifests);

                crate::package::triggers::run_triggers("install", &[name.to_string()]);

                return Ok(());
            }
        }

        Err(SpmError::package_not_found(format!(
            "Package '{name}' found in repo metadata but failed to download"
        )))
    }

    pub(crate) fn execute_smart(&self, plan: TransactionPlan, replace: bool, smart: bool) -> SpmResult<()> {
        let repos_list = repos::load_repos()?;
        let mut removed: Vec<String> = plan.to_remove.clone();
        let mut all_file_conflicts: HashMap<String, Vec<String>> = HashMap::new();

        // Collect store hashes of currently installed dependencies (before any removals)
        let dep_hashes: Vec<String> = plan.all_packages.iter()
            .filter(|p| p.name != plan.name)
            .filter_map(|p| db::get_store_hash(self.conn, &p.name, &p.format).ok().flatten())
            .collect();

        // ═══════════════════════════════════════
        // Phase A: Fetch & Detect
        // ═══════════════════════════════════════

        let mut fetched_list: Vec<fetch::FetchedPackage> = Vec::new();

        for pid in plan.all_packages.iter() {
            let name = &pid.name;
            let is_main = name == &plan.name;

            if db::is_installed(self.conn, name) && !is_main {
                crate::output::step_info(format!("Skipping '{}' (already installed)", name));
                continue;
            }

            crate::output::step_info(format!(
                "{} '{}' ({:?})...",
                if is_main { "Fetching" } else { "Fetching dependency" },
                name,
                pid.format,
            ));

            let matching_source = match pid.format {
                PackageFormat::Deb => RepoSource::Deb,
                PackageFormat::Rpm => RepoSource::Rpm,
                PackageFormat::Sam => RepoSource::Native,
            };

            let fetched = self.fetch_single(name, &matching_source, &repos_list, replace)?;

            // Store trigger scripts for SAM v2 triggers
            if !fetched.manifest.triggers.is_empty() {
                let _ = crate::package::triggers::store_triggers(
                    name, &fetched.manifest, &fetched.extracted_dir,
                );
            }

            // Process obsoletes from SAM v2 manifest
            for obsolete_pkg in &fetched.manifest.obsoletes {
                if db::is_installed(self.conn, obsolete_pkg) && !removed.contains(obsolete_pkg) {
                    crate::output::step_info(format!(
                        "Package '{}' obsoletes '{}' — will be removed",
                        name, obsolete_pkg
                    ));
                    removed.push(obsolete_pkg.clone());
                }
            }

            // Detect file-level conflicts after extraction
            let new_files: HashSet<String> = fetched.files.iter()
                .filter(|f| matches!(f.action, FileAction::Created))
                .map(|f| f.filepath.clone())
                .collect();

            let file_conflicts = if !new_files.is_empty() {
                conflict::detect_file_conflicts(self.conn, &new_files, &removed)?
            } else {
                HashMap::new()
            };

            if !file_conflicts.is_empty() {
                if replace {
                    for (pkg, files) in &file_conflicts {
                        if !removed.contains(pkg) {
                            crate::output::step_info(format!(
                                "Auto-removing conflicting '{}' ({} overlapping files)",
                                pkg,
                                files.len(),
                            ));
                            removed.push(pkg.clone());
                            all_file_conflicts.entry(pkg.clone()).or_default().extend(files.clone());
                        }
                    }
                } else {
                    // Error before any FS or DB changes — clean
                    let names: Vec<&str> = file_conflicts.keys().map(|s| s.as_str()).collect();
                    return Err(SpmError::other(format!(
                        "File conflicts detected with: {}. Use --replace to force install.",
                        names.join(", "),
                    )));
                }
            }

            fetched_list.push(fetched);
        }

        // ═══════════════════════════════════════
        // Phase B: FS Deploy (with RollbackGuard)
        // ═══════════════════════════════════════

        let mut guards: Vec<RollbackGuard> = Vec::new();
        for fetched in &mut fetched_list {
            guards.push(self.fs_deploy_one(fetched, &dep_hashes, smart)?);
        }

        // Capture remove info BEFORE DB transaction (files table will be cleared)
        let remove_infos = self.capture_remove_infos(&removed);

        // ═══════════════════════════════════════
        // Phase C: DB Atomic Commit
        // ═══════════════════════════════════════

        let db_result = self.atomic_commit(&plan, &mut fetched_list, &removed, &all_file_conflicts);

        match db_result {
            Ok(()) => {
                // ═══════════════════════════════
                // Phase D: Finalize
                // ═══════════════════════════════
                for g in &mut guards {
                    g.disarm();
                }

                for fetched in &fetched_list {
                    self.run_package_scripts(fetched);
                }

                if !removed.is_empty() {
                    crate::output::step_info(format!("Cleaning up {} removed packages...", removed.len()));
                }
                for (name, files, store_hash, install_type, manifest, format) in &remove_infos {
                    let _ = self.cleanup_removed(name, files, store_hash.as_deref(), install_type, manifest.as_ref(), format);
                }

                let all_files: Vec<String> = fetched_list.iter()
                    .flat_map(|f| f.files.iter().map(|fr| fr.filepath.clone()))
                    .collect();
                crate::package::hooks::run_install_hooks(&all_files);

                let installed_names: Vec<String> = fetched_list.iter()
                    .map(|f| f.pkg.name.clone())
                    .collect();
                crate::package::hooks::run_kernel_hooks(&installed_names);

                let manifests: Vec<&crate::types::Manifest> = fetched_list.iter()
                    .map(|f| &f.manifest)
                    .collect();
                crate::package::hooks::run_sam_v2_hooks(&manifests);

                // Run triggers matching newly installed packages
                crate::package::triggers::run_triggers("install", &installed_names);
                if !removed.is_empty() {
                    crate::package::triggers::run_triggers("remove", &removed);
                }

                crate::output::step_info("Installation complete");
                Ok(())
            }
            Err(e) => {
                // Guards dropped → RollbackGuard::drop() cleans store + symlinks
                // DB automatically rolled back — no atomic_commit succeeded
                Err(e)
            }
        }
    }

    // ═══════════════════════════════════════════
    // Phase A helpers
    // ═══════════════════════════════════════════

    fn fetch_single(
        &self,
        name: &str,
        matching_source: &RepoSource,
        repos_list: &[(String, RepoConfig)],
        replace: bool,
    ) -> SpmResult<fetch::FetchedPackage> {
        // Try matching repos first, then Native repos as fallback
        for (rn, rc) in repos_list {
            if rc.source == *matching_source {
                if let Ok(fetched) = fetch::fetch_and_extract(name, rn, rc, replace, self.conn) {
                    return Ok(fetched);
                }
            }
        }
        // Fallback: try Native repos for any format
        for (rn, rc) in repos_list {
            if rc.source == RepoSource::Native && *matching_source != RepoSource::Native {
                if let Ok(fetched) = fetch::fetch_and_extract(name, rn, rc, replace, self.conn) {
                    return Ok(fetched);
                }
            }
        }
        Err(SpmError::package_not_found(format!(
            "Package '{name}' ({matching_source:?}) not found in any matching repository",
        )))
    }

    // ═══════════════════════════════════════════
    // Phase B: FS deploy (guarded)
    // ═══════════════════════════════════════════

    fn fs_deploy_one(
        &self,
        fetched: &mut fetch::FetchedPackage,
        dep_store_hashes: &[String],
        smart: bool,
    ) -> SpmResult<RollbackGuard> {
        let name = fetched.pkg.name.clone();
        let mut guard = RollbackGuard::new();
        let data_dir = Path::new(&fetched.extracted_dir);
        let origin = store::origin_from_format(&fetched.pkg.format);

        if data_dir.exists() {
            let pkg_hash = hash::hash_dir(data_dir)?;
            let store_path = store::copy_to_store_with_origin(data_dir, &pkg_hash, origin)?;
            guard.set_store_hash(pkg_hash.clone());
            guard.set_store_origin(origin.to_string());

            let dep_store_dirs: Vec<PathBuf> = dep_store_hashes.iter()
                .map(|h| store::store_package_dir_for_origin(h, origin))
                .filter(|d| d.exists())
                .collect();
            store::set_rpath_on_elfs(&store_path, &dep_store_dirs)?;

            let symlinks = if smart {
                store::create_fhs_symlinks_smart(&store_path)?
            } else {
                store::create_fhs_symlinks(&store_path)?
            };
            guard.set_symlinks(symlinks);

            fetched.pkg.store_hash = Some(pkg_hash);
        }

        if !fetched.scripts.is_empty() {
            scripts::save_scripts(&name, &fetched.scripts)?;
        }

        let _ = fs::remove_dir_all(&fetched.extracted_dir);

        Ok(guard)
    }

    // ═══════════════════════════════════════════
    // Run scripts after DB commit (Phase D)
    // ═══════════════════════════════════════════

    fn run_package_scripts(&self, fetched: &fetch::FetchedPackage) {
        if let Some(ref script) = fetched.scripts.preinst {
            crate::output::step_info(format!("Running preinst script for {}", fetched.pkg.name));
            let _ = crate::package::scripts::run_script(script, "install");
        }
        if let Some(ref script) = fetched.scripts.postinst {
            crate::output::step_info(format!("Running postinst script for {}", fetched.pkg.name));
            let _ = crate::package::scripts::run_script(script, "configure");
        }
    }

    // ═══════════════════════════════════════════
    // Capture remove info (read from DB, before deletion)
    // ═══════════════════════════════════════════

    #[allow(clippy::type_complexity)]
    fn capture_remove_infos(&self, removed: &[String]) -> Vec<(String, Vec<FileRecord>, Option<String>, InstallType, Option<Manifest>, PackageFormat)> {
        let pkgs = db::get_installed_packages_batch(self.conn, removed).unwrap_or_default();
        let pkg_map: std::collections::HashMap<&str, (Option<String>, InstallType, Option<Manifest>, PackageFormat)> = pkgs.iter()
            .map(|(name, pkg)| {
                let manifest = pkg.manifest.as_ref().and_then(|m| serde_json::from_str(m).ok());
                (name.as_str(), (pkg.store_hash.clone(), pkg.install_type.clone(), manifest, pkg.format.clone()))
            })
            .collect();
        let all_files = db::get_files_by_packages_batch(self.conn, removed).unwrap_or_default();
        removed.iter().filter_map(|name| {
            let (store_hash, install_type, manifest, format) = pkg_map.get(name.as_str()).cloned()?;
            let files: Vec<FileRecord> = all_files.iter().filter(|f| f.package == *name).cloned().collect();
            Some((name.clone(), files, store_hash, install_type, manifest, format))
        }).collect()
    }

    // ═══════════════════════════════════════════
    // Phase C: Atomic DB commit
    // ═══════════════════════════════════════════

    fn atomic_commit(
        &self,
        plan: &TransactionPlan,
        fetched_list: &mut [fetch::FetchedPackage],
        removed: &[String],
        all_file_conflicts: &HashMap<String, Vec<String>>,
    ) -> SpmResult<()> {
        self.conn.execute_batch("BEGIN")?;

        let result = self.commit_inner(plan, fetched_list, removed, all_file_conflicts);

        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                self.conn.execute_batch("ROLLBACK")?;
                Err(e)
            }
        }
    }

    fn commit_inner(
        &self,
        plan: &TransactionPlan,
        fetched_list: &mut [fetch::FetchedPackage],
        removed: &[String],
        all_file_conflicts: &HashMap<String, Vec<String>>,
    ) -> SpmResult<()> {
        // Record ONE transaction for the entire batch
        let all_pkg_names: Vec<String> = plan.all_packages.iter().map(|p| p.name.clone()).collect();
        let batch_tx_id = db::record_transaction(self.conn, &Transaction {
            id: None,
            action: TransactionAction::Install,
            timestamp: Utc::now().to_rfc3339(),
            user: crate::util::fs::whoami(),
            status: TransactionStatus::Completed,
            packages: all_pkg_names,
            snapshot_id: None,
        })?;

        // Remove conflicting packages from DB (ALL in this one SQLite TX)
        for name in removed {
            if let Some(pkg) = db::get_installed_package(self.conn, name)? {
                let pid = PackageId::new(name, pkg.format);
                db::remove_installed_package_by_id(self.conn, &pid)?;
            }
        }

        // Record each new package in DB
        for fetched in fetched_list.iter() {
            let files_with_tx: Vec<FileRecord> = fetched.files.iter().map(|f| {
                let mut f2 = f.clone();
                f2.transaction_id = batch_tx_id;
                f2
            }).collect();

            if !files_with_tx.is_empty() {
                db::record_files(self.conn, &files_with_tx)?;
            }
            db::add_installed_package(self.conn, &fetched.pkg)?;
        }

        // Record conflict_log for file-level conflicts
        if !all_file_conflicts.is_empty() {
            conflict::record_conflicts_batch(self.conn, batch_tx_id, all_file_conflicts, &plan.name)?;
        }

        Ok(())
    }

    // ═══════════════════════════════════════════
    // Phase D: FS cleanup after DB commit
    // ═══════════════════════════════════════════

    fn cleanup_removed(
        &self,
        name: &str,
        _file_records: &[FileRecord],
        store_hash: Option<&str>,
        install_type: &InstallType,
        manifest: Option<&Manifest>,
        format: &PackageFormat,
    ) -> SpmResult<()> {
        let scripts_data = crate::package::scripts::load_scripts(name).unwrap_or_default();

        if let Some(ref script) = scripts_data.prerm {
            let _ = crate::package::scripts::run_script(script, "remove");
        }

        // Don't delete files by path — they may now belong to the replacement package.
        // Only clean up the old store by hash (which is unique per package version).
        if let Some(hash) = store_hash {
            let origin = store::origin_from_format(format);
            let _ = store::gc_store_with_origin(self.conn, hash, origin);
        }

        if matches!(install_type, InstallType::Sandbox) {
            let sandbox_dir = crate::config::paths::sandbox_dir(name);
            if sandbox_dir.exists() {
                let _ = fs::remove_dir_all(&sandbox_dir);
            }
        }

        // SAM v2 cleanup: disable systemd units, remove sysusers/tmpfiles confs
        if let Some(m) = manifest {
            crate::package::hooks::remove_sam_v2_hooks(m);
        }

        let _ = crate::package::scripts::remove_scripts(name);

        if let Some(ref script) = scripts_data.postrm {
            let _ = crate::package::scripts::run_script(script, "remove");
        }

        Ok(())
    }
}

use crate::db::conflict::ConflictSummary;

/// Backup user-modified conffiles before a package upgrade deploys new versions.
/// For each conffile in the new package:
///   - If file doesn't exist on disk → skip (fresh install)
///   - If disk hash matches old package hash → user didn't modify → will be replaced silently
///   - If disk hash matches new package hash → already up to date → skip
///   - Otherwise → user modified AND package changed → save as .rpmsave
fn backup_conffiles_for_upgrade(
    old_files: &[FileRecord],
    new_manifest: &Manifest,
    extracted_dir: &str,
) {
    let conffiles = &new_manifest.conffiles;
    if conffiles.is_empty() {
        return;
    }

    let old_hashes: HashMap<&str, &str> = old_files.iter()
        .map(|f| (f.filepath.as_str(), f.hash.as_str()))
        .collect();

    for conffile in conffiles {
        let path = Path::new(conffile);
        if !path.exists() {
            continue;
        }

        let disk_hash = match hash::hash_file(conffile) {
            Ok(h) => h,
            Err(_) => continue,
        };

        let old_hash = match old_hashes.get(conffile.as_str()) {
            Some(h) => h,
            None => continue,
        };

        let new_file_path = Path::new(extracted_dir).join(conffile.trim_start_matches('/'));
        let new_hash = match hash::hash_file(&new_file_path.to_string_lossy()) {
            Ok(h) => h,
            Err(_) => continue,
        };

        if *old_hash == disk_hash || new_hash == disk_hash {
            continue;
        }

        let backup_path = format!("{}.rpmsave", conffile);
        if let Err(e) = fs::copy(conffile, &backup_path) {
            tracing::warn!("Failed to backup conffile '{}': {e}", conffile);
            continue;
        }
        crate::output::step_info(format!(
            "Conffile '{}' was modified locally — saved as '{}'",
            conffile, backup_path,
        ));
    }
}

// ═══════════════════════════════════════════
// RollbackGuard
// ═══════════════════════════════════════════

pub(crate) struct RollbackGuard {
    store_hash: Option<String>,
    store_origin: String,
    symlinks: Vec<store::SymlinkRecord>,
    armed: bool,
}

impl RollbackGuard {
    pub(crate) fn new() -> Self {
        Self {
            store_hash: None,
            store_origin: String::new(),
            symlinks: Vec::new(),
            armed: true,
        }
    }

    pub(crate) fn set_store_hash(&mut self, hash: String) {
        self.store_hash = Some(hash);
    }

    pub(crate) fn set_store_origin(&mut self, origin: String) {
        self.store_origin = origin;
    }

    pub(crate) fn set_symlinks(&mut self, symlinks: Vec<store::SymlinkRecord>) {
        self.symlinks = symlinks;
    }

    pub(crate) fn disarm(&mut self) {
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
                // Only remove parent dir if it's empty — guards against
                // TOCTOU races with concurrent transactions.
                if parent.exists() {
                    let empty = parent.read_dir()
                        .map(|mut i| i.next().is_none())
                        .unwrap_or(false);
                    if empty {
                        let _ = fs::remove_dir(parent);
                    }
                }
            }
        }
        if let Some(ref hash) = self.store_hash {
            let store_dir = if self.store_origin.is_empty() {
                store::store_package_dir(hash)
            } else {
                store::store_package_dir_for_origin(hash, &self.store_origin)
            };
            if store_dir.exists() {
                let _ = fs::remove_dir_all(&store_dir);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_conflicts() -> HashMap<String, Vec<String>> {
        let mut map = HashMap::new();
        map.insert("coreutils".to_string(), vec![
            "/usr/bin/ls".to_string(),
            "/usr/bin/cat".to_string(),
            "/usr/bin/rm".to_string(),
            "/usr/bin/mv".to_string(),
            "/usr/bin/cp".to_string(),
            "/usr/bin/mkdir".to_string(),
            "/usr/bin/ln".to_string(),
        ]);
        map.insert("bash".to_string(), vec![
            "/usr/bin/bash".to_string(),
        ]);
        map.insert("man-pages".to_string(), vec![
            "/usr/share/man/man1/test.1.gz".to_string(),
        ]);
        map
    }

    #[test]
    fn test_classify_critical() {
        let map = make_conflicts();
        let (critical, shared, minor) = conflict::classify_conflicts(&map);
        assert_eq!(critical.len(), 1, "coreutils should be critical");
        assert_eq!(critical[0].package, "coreutils");
        assert_eq!(shared.len(), 0);
        assert_eq!(minor.len(), 2);
    }

    #[test]
    fn test_rollback_guard_disarm() {
        let mut guard = RollbackGuard::new();
        guard.disarm();
        // Should not panic on drop
    }

    #[test]
    fn test_rolling_guard_drop_noop() {
        let guard = RollbackGuard {
            store_hash: None,
            store_origin: String::new(),
            symlinks: Vec::new(),
            armed: true,
        };
        drop(guard); // armed but no store/symlinks → no-op
    }
}
