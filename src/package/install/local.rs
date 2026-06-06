use std::path::Path;

use crate::config::paths;
use crate::db;
use crate::error::{SpmError, SpmResult};
use crate::package::fetch;
use crate::package::transaction::TransactionEngine;
use crate::types::*;
use crate::util::hash;

pub fn install_local_package(path: &str, replace: bool, yes: bool) -> SpmResult<()> {
    let tmp_dir_obj = tempfile::tempdir()
        .map_err(|e| SpmError::other(format!("Failed to create temp directory: {e}")))?;
    let tmp_dir = tmp_dir_obj.path().to_string_lossy().to_string();

    let mut fetched = fetch::fetch_local_to_temp(path, &tmp_dir)?;
    let name = fetched.pkg.name.clone();

    crate::output::section(format!("📦 Installing {} v{}", name, fetched.pkg.version));

    let scanned_manifest = fetch::build_manifest(&fetched.pkg, Some(&fetched.extracted_dir))?;

    // Preserve SAM v2 fields from the original manifest
    if !fetched.manifest.systemd_units.is_empty()
        || !fetched.manifest.sysusers.is_empty()
        || !fetched.manifest.tmpfiles.is_empty()
        || !fetched.manifest.triggers.is_empty()
        || !fetched.manifest.obsoletes.is_empty()
        || fetched.manifest.format_version > 1
        || fetched.manifest.source.is_some()
    {
        let mut merged = scanned_manifest;
        merged.systemd_units = std::mem::take(&mut fetched.manifest.systemd_units);
        merged.sysusers = std::mem::take(&mut fetched.manifest.sysusers);
        merged.tmpfiles = std::mem::take(&mut fetched.manifest.tmpfiles);
        merged.triggers = std::mem::take(&mut fetched.manifest.triggers);
        merged.obsoletes = std::mem::take(&mut fetched.manifest.obsoletes);
        merged.format_version = merged.format_version.max(fetched.manifest.format_version);
        merged.source = fetched.manifest.source.take().or(merged.source);
        merged.signature = fetched.manifest.signature.take().or(merged.signature);
        fetched.manifest = merged;
    } else {
        fetched.manifest = scanned_manifest;
    }

    // Scan extracted directory for files (no DB conflict check needed here)
    let files = scan_dir_for_records(&fetched.extracted_dir, &name, &fetched.pkg.format)?;
    let filepaths: Vec<String> = files.iter().map(|f| f.filepath.clone()).collect();
    fetched.files = files;

    // Create SAM archive
    let sam_path = paths::sam_archive(&name);
    let sam_result = crate::package::sam::create_sam(
        &fetched.manifest, &fetched.extracted_dir, None, &sam_path.to_string_lossy(),
    );
    if let Err(e) = sam_result {
        tracing::warn!("Failed to create .sam for {}: {}. Installing without .sam cache.", name, e);
    }

    // SAM conversion: always store as Sam format
    fetched.pkg.format = PackageFormat::Sam;

    // Phase 0: Plan
    let plan = db::with_read_lock(|conn| {
        if db::is_installed(conn, &name) && !replace {
            return Err(SpmError::package_already_installed(&name));
        }
        let engine = TransactionEngine::new(conn);
        engine.plan_install_local(&name, replace, &filepaths)
    })?;

    // Phase 1: Display + Approve
    TransactionEngine::display_plan(&plan);
    if !TransactionEngine::approve_plan(&plan, yes)? {
        return Err(SpmError::other(format!("Installation of '{}' cancelled.", name)));
    }

    // Phase 2: Execute
    db::with_write_lock(|conn| {
        let engine = TransactionEngine::new(conn);
        engine.install_local(&mut fetched, replace)
    })
}

/// Scans an extracted directory and creates FileRecords (without DB conflict detection)
fn scan_dir_for_records(
    extracted_dir: &str,
    pkg_name: &str,
    pkg_format: &PackageFormat,
) -> SpmResult<Vec<FileRecord>> {
    let base = Path::new(extracted_dir);
    if !base.exists() {
        return Ok(Vec::new());
    }

    let entries: Vec<_> = walkdir::WalkDir::new(base)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_type().is_dir())
        .collect();

    let mut records = Vec::with_capacity(entries.len());

    for entry in &entries {
        let relative = match entry.path().strip_prefix(base) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let target_str = format!("/{}", relative.display());

        let file_hash = if entry.file_type().is_symlink() {
            match std::fs::read_link(entry.path()) {
                Ok(link) => hash::hash_bytes(link.to_string_lossy().as_bytes()),
                Err(_) => continue,
            }
        } else {
            match entry.path().to_str() {
                Some(s) => match hash::hash_file(s) {
                    Ok(h) => h,
                    Err(_) => continue,
                },
                None => continue,
            }
        };

        records.push(FileRecord {
            id: None,
            transaction_id: 0,
            package: pkg_name.to_string(),
            format: pkg_format.clone(),
            filepath: target_str,
            hash: file_hash,
            action: FileAction::Created,
        });
    }

    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_scan_dir_for_records_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let records = scan_dir_for_records(dir.path().to_str().unwrap(), "test-pkg", &PackageFormat::Sam).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_scan_dir_for_records_with_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("usr/bin")).unwrap();
        fs::create_dir_all(dir.path().join("etc")).unwrap();
        fs::write(dir.path().join("usr/bin/hello"), b"binary").unwrap();
        fs::write(dir.path().join("etc/hi.conf"), b"config").unwrap();

        let records = scan_dir_for_records(dir.path().to_str().unwrap(), "test-pkg", &PackageFormat::Sam).unwrap();
        assert_eq!(records.len(), 2);

        let mut paths: Vec<_> = records.iter().map(|r| r.filepath.clone()).collect();
        paths.sort();
        assert_eq!(paths[0], "/etc/hi.conf");
        assert_eq!(paths[1], "/usr/bin/hello");

        for r in &records {
            assert_eq!(r.package, "test-pkg");
            assert_eq!(r.format, PackageFormat::Sam);
            assert_eq!(r.action, FileAction::Created);
        }
    }

    #[test]
    fn test_scan_dir_for_records_skips_dirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("usr/share")).unwrap();
        // Create a subdirectory (should be skipped) and a file inside it
        let inner_dir = dir.path().join("usr/share/emptydir");
        fs::create_dir_all(&inner_dir).unwrap();
        fs::write(dir.path().join("usr/share/file.txt"), b"data").unwrap();

        let records = scan_dir_for_records(dir.path().to_str().unwrap(), "pkg", &PackageFormat::Deb).unwrap();
        // Only the file is returned, not the directory
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].filepath, "/usr/share/file.txt");
    }

    #[test]
    fn test_scan_dir_for_records_hash_present() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("data.bin"), b"hello world").unwrap();
        let records = scan_dir_for_records(dir.path().to_str().unwrap(), "pkg", &PackageFormat::Rpm).unwrap();
        assert_eq!(records.len(), 1);
        assert!(!records[0].hash.is_empty());
    }

    #[test]
    fn test_scan_dir_for_records_nonexistent_dir() {
        let records = scan_dir_for_records("/nonexistent-path-xyz", "pkg", &PackageFormat::Sam).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_scan_dir_for_records_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("usr/bin/real");
        let link = dir.path().join("usr/bin/link");
        fs::create_dir_all(dir.path().join("usr/bin")).unwrap();
        fs::write(&target, b"real").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let records = scan_dir_for_records(dir.path().to_str().unwrap(), "pkg", &PackageFormat::Sam).unwrap();
        assert_eq!(records.len(), 2);

        let link_record = records.iter().find(|r| r.filepath == "/usr/bin/link").unwrap();
        assert!(!link_record.hash.is_empty());
    }
}
