use std::collections::HashSet;

use crate::db;
use crate::error::SpmResult;

pub struct DaemonConflictHandler;

impl DaemonConflictHandler {
    pub fn new() -> Self {
        Self
    }

    pub fn check_conflicts(
        &self,
        _package_name: &str,
        files: &[String],
    ) -> SpmResult<Vec<ConflictEntry>> {
        let file_set: HashSet<String> = files.iter().cloned().collect();
        let conflicts = db::with_read_lock(|conn| {
            db::conflict::detect_file_conflicts(conn, &file_set, &[])
        })?;

        let mut entries = Vec::new();
        for (pkg, file_list) in conflicts {
            for file in file_list {
                let severity = if pkg.starts_with("lib") {
                    ConflictSeverity::Shared
                } else {
                    ConflictSeverity::Critical
                };
                entries.push(ConflictEntry {
                    file,
                    existing_package: pkg.clone(),
                    severity,
                });
            }
        }
        Ok(entries)
    }

    pub fn resolve(&self, conflicts: &[ConflictEntry]) -> SpmResult<ConflictResolution> {
        let has_critical = conflicts
            .iter()
            .any(|c| matches!(c.severity, ConflictSeverity::Critical));
        if has_critical {
            let mut packages: Vec<String> = conflicts
                .iter()
                .map(|c| c.existing_package.clone())
                .collect();
            packages.sort();
            packages.dedup();
            Ok(ConflictResolution::Replace(packages))
        } else {
            Ok(ConflictResolution::Allow)
        }
    }
}

pub struct ConflictEntry {
    pub file: String,
    pub existing_package: String,
    pub severity: ConflictSeverity,
}

pub enum ConflictSeverity {
    Critical,
    Shared,
    Minor,
}

pub enum ConflictResolution {
    Allow,
    Block,
    Replace(Vec<String>),
}
