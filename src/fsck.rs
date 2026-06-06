use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::types::*;

#[derive(Debug, Default)]
struct FsckReport {
    integrity_ok: bool,
    #[allow(dead_code)]
    orphan_files: Vec<String>,
    #[allow(dead_code)]
    missing_files: Vec<(String, String)>,
    #[allow(dead_code)]
    dangling_refs: Vec<String>,
    total_issues: usize,
    fixed: usize,
}

pub fn check_integrity(fix: bool, check_files: bool) -> SpmResult<()> {
    let mut report = FsckReport::default();

    crate::output::section("🔍 Database integrity check");
    db::with_write_lock(|conn| {
        // Phase 1: SQLite integrity check
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap_or_else(|_| "error".to_string());
        report.integrity_ok = integrity == "ok";
        if !report.integrity_ok {
            report.total_issues += 1;
            crate::output::step_warn(format!("Database corruption detected: {}", integrity));
            if fix {
                // SQLite's own recovery is limited; we can attempt a VACUUM
                conn.execute("VACUUM", [])?;
                let retry: String = conn
                    .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                    .unwrap_or_else(|_| "error".to_string());
                if retry == "ok" {
                    report.fixed += 1;
                    crate::output::step_info("Database repaired via VACUUM");
                } else {
                    return Err(SpmError::other(
                        "Database corruption cannot be auto-repaired. Restore from backup."
                    ));
                }
            }
        } else {
            crate::output::step_info("Database integrity: OK");
        }

        // Phase 2: Check for packages with missing file records
        let all_pkgs = db::list_installed_packages(conn)?;
        let all_files = db::get_all_files(conn)?;
        let mut files_by_pkg: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for f in &all_files {
            *files_by_pkg.entry(f.package.clone()).or_insert(0) += 1;
        }

        let mut missing_pkg_files = Vec::new();
        for pkg in &all_pkgs {
            let file_count = files_by_pkg.get(&pkg.name).copied().unwrap_or(0);
            if file_count == 0 && !matches!(pkg.origin, InstallOrigin::Foreign) {
                missing_pkg_files.push(pkg.name.clone());
            }
        }
        if !missing_pkg_files.is_empty() {
            report.total_issues += missing_pkg_files.len();
            crate::output::step_warn(format!(
                "{} packages have zero file records", missing_pkg_files.len()
            ));
        }

        // Phase 3: Check for orphan files (files belonging to removed packages)
        let installed_names: std::collections::HashSet<String> =
            all_pkgs.iter().map(|p| p.name.clone()).collect();
        let mut file_packages: std::collections::HashSet<String> = std::collections::HashSet::new();
        for f in &all_files {
            file_packages.insert(f.package.clone());
        }
        for pkg_name in &file_packages {
            if !installed_names.contains(pkg_name) {
                report.orphan_files.push(pkg_name.clone());
                report.total_issues += 1;
            }
        }
        if !report.orphan_files.is_empty() {
            crate::output::step_warn(format!(
                "{} orphan file groups (files for removed packages)", report.orphan_files.len()
            ));
            if fix {
                for pkg_name in &report.orphan_files {
                    db::remove_installed_package(conn, pkg_name)?;
                    report.fixed += 1;
                }
                crate::output::step_info(format!("Removed {} orphan groups", report.orphan_files.len()));
            }
        }

        // Phase 4: Verify file existence on disk
        if check_files {
            crate::output::step_info("Checking file existence on disk...");
            let mut missing = 0;
            for f in &all_files {
                let path = std::path::Path::new(&f.filepath);
                if !path.exists() {
                    report.missing_files.push((f.package.clone(), f.filepath.clone()));
                    missing += 1;
                    report.total_issues += 1;
                }
            }
            if missing > 0 {
                crate::output::step_warn(format!("{} recorded files missing from disk", missing));
                if fix {
                    for (pkg_name, fp) in &report.missing_files {
                        println!("  Missing: {} owned by {}", fp, pkg_name);
                    }
                    crate::output::step_info("Run `spm install --replace <pkg>` to restore missing files");
                }
            } else {
                crate::output::step_info("All recorded files present on disk");
            }
        }

        // Phase 5: Check for dangling transaction references
        let orphan_tx: Vec<String> = conn
            .prepare("SELECT DISTINCT f.package FROM files f LEFT JOIN installed_packages p ON f.package = p.name WHERE p.name IS NULL")?
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        report.dangling_refs = orphan_tx;
        if !report.dangling_refs.is_empty() {
            report.total_issues += report.dangling_refs.len();
            crate::output::step_warn(format!(
                "{} dangling file references", report.dangling_refs.len()
            ));
        }

        Ok(())
    })?;

    // Summary
    println!();
    if report.total_issues == 0 {
        crate::output::result_message("No issues found. Database is clean.");
    } else {
        crate::output::step_warn(format!(
            "Found {} issue(s), {} fixed",
            report.total_issues, report.fixed
        ));
    }

    Ok(())
}
