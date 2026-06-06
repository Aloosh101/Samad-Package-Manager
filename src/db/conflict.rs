use std::collections::{HashMap, HashSet};
use chrono::Utc;

use crate::error::SpmResult;

#[allow(dead_code)]
pub(crate) struct ConflictLogEntry {
    pub(crate) transaction_id: i64,
    pub(crate) package_a: String,
    pub(crate) package_b: String,
    pub(crate) file_path: String,
    pub(crate) action: String,
    pub(crate) timestamp: String,
}

pub(crate) struct ConflictSummary {
    pub(crate) package: String,
    #[allow(dead_code)]
    pub(crate) version: String,
    pub(crate) file_count: usize,
    #[allow(dead_code)]
    pub(crate) severity: ConflictSeverity,
    pub(crate) reason: String,
}

#[derive(Debug, PartialEq)]
pub(crate) enum ConflictSeverity {
    Critical,
    Shared,
    Minor,
}

pub(crate) fn init_conflict_schema(conn: &rusqlite::Connection) -> SpmResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS conflict_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            transaction_id INTEGER NOT NULL,
            package_a TEXT NOT NULL,
            package_b TEXT NOT NULL,
            file_path TEXT NOT NULL,
            action TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            FOREIGN KEY (transaction_id) REFERENCES transactions(id)
        );

        CREATE INDEX IF NOT EXISTS idx_conflict_log_a ON conflict_log(package_a);
        CREATE INDEX IF NOT EXISTS idx_conflict_log_b ON conflict_log(package_b);
        CREATE INDEX IF NOT EXISTS idx_conflict_tx ON conflict_log(transaction_id);",
    )?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn record_conflict(conn: &rusqlite::Connection, entry: &ConflictLogEntry) -> SpmResult<i64> {
    conn.execute(
        "INSERT INTO conflict_log (transaction_id, package_a, package_b, file_path, action, timestamp) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            entry.transaction_id,
            entry.package_a,
            entry.package_b,
            entry.file_path,
            entry.action,
            entry.timestamp,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub(crate) fn record_conflicts_batch(
    conn: &rusqlite::Connection,
    tx_id: i64,
    conflicts: &HashMap<String, Vec<String>>,
    replacement_pkg: &str,
) -> SpmResult<()> {
    let now = Utc::now().to_rfc3339();
    for (pkg_a, files) in conflicts {
        for file_path in files {
            conn.execute(
                "INSERT INTO conflict_log (transaction_id, package_a, package_b, file_path, action, timestamp) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![tx_id, pkg_a, replacement_pkg, file_path, "replaced", now],
            )?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn get_conflicts_by_transaction(
    conn: &rusqlite::Connection,
    tx_id: i64,
) -> SpmResult<Vec<ConflictLogEntry>> {
    let mut stmt = conn.prepare(
        "SELECT transaction_id, package_a, package_b, file_path, action, timestamp \
         FROM conflict_log WHERE transaction_id = ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![tx_id], |row| {
        Ok(ConflictLogEntry {
            transaction_id: row.get(0)?,
            package_a: row.get(1)?,
            package_b: row.get(2)?,
            file_path: row.get(3)?,
            action: row.get(4)?,
            timestamp: row.get(5)?,
        })
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

#[allow(dead_code)]
pub(crate) fn get_conflicts_for_package(
    conn: &rusqlite::Connection,
    package: &str,
) -> SpmResult<Vec<ConflictLogEntry>> {
    let mut stmt = conn.prepare(
        "SELECT transaction_id, package_a, package_b, file_path, action, timestamp \
         FROM conflict_log WHERE package_a = ?1 OR package_b = ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![package], |row| {
        Ok(ConflictLogEntry {
            transaction_id: row.get(0)?,
            package_a: row.get(1)?,
            package_b: row.get(2)?,
            file_path: row.get(3)?,
            action: row.get(4)?,
            timestamp: row.get(5)?,
        })
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Detect file-level conflicts using HashSet intersection.
/// Returns: HashMap<conflicting_package_name, vec![overlapping_file_paths]>
pub(crate) fn detect_file_conflicts(
    conn: &rusqlite::Connection,
    new_files: &HashSet<String>,
    exclude_packages: &[String],
) -> SpmResult<HashMap<String, Vec<String>>> {
    let mut conflicts: HashMap<String, Vec<String>> = HashMap::new();

    // Build inverted index from DB: filepath → vec![package names]
    // We query all files except those from packages being replaced
    let mut stmt = if exclude_packages.is_empty() {
        conn.prepare("SELECT package, filepath FROM files")?
    } else {
        let placeholders: Vec<String> = exclude_packages.iter().enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        conn.prepare(&format!(
            "SELECT package, filepath FROM files WHERE package NOT IN ({})",
            placeholders.join(","),
        ))?
    };

    let params: Vec<&dyn rusqlite::types::ToSql> = exclude_packages.iter()
        .map(|s| s as &dyn rusqlite::types::ToSql)
        .collect();
    let rows = stmt.query_map(params.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
        ))
    })?;

    for row in rows {
        let (pkg, filepath) = row?;
        if new_files.contains(&filepath) {
            conflicts.entry(pkg).or_default().push(filepath);
        }
    }

    Ok(conflicts)
}

/// Classify conflicts by severity based on file paths.
///
/// Returns (critical, shared, minor) where:
/// - Critical: packages with bin/lib files and >5 overlapping files
/// - Shared: packages with >3 overlapping files
/// - Minor: packages with ≤3 overlapping files (auto-resolvable)
pub(crate) fn classify_conflicts(
    conflicts: &HashMap<String, Vec<String>>,
) -> (Vec<ConflictSummary>, Vec<ConflictSummary>, Vec<ConflictSummary>) {
    let mut critical = Vec::new();
    let mut shared = Vec::new();
    let mut minor = Vec::new();

    for (pkg, files) in conflicts {
        let count = files.len();
        let has_bin = files.iter().any(|f| f.starts_with("/usr/bin/") || f.starts_with("/bin/"));
        let has_lib = files.iter().any(|f| f.contains("/lib"));

        if (has_bin || has_lib) && count > 5 {
            let sample: Vec<&str> = files.iter().take(3).map(|s| s.as_str()).collect();
            critical.push(ConflictSummary {
                package: pkg.clone(),
                version: String::new(),
                file_count: count,
                severity: ConflictSeverity::Critical,
                reason: format!("bin/lib overlap: {:?}", sample),
            });
        } else if count > 3 {
            shared.push(ConflictSummary {
                package: pkg.clone(),
                version: String::new(),
                file_count: count,
                severity: ConflictSeverity::Shared,
                reason: format!("{} shared files", count),
            });
        } else {
            minor.push(ConflictSummary {
                package: pkg.clone(),
                version: String::new(),
                file_count: count,
                severity: ConflictSeverity::Minor,
                reason: format!("{} shared files", count),
            });
        }
    }

    // Sort by file count descending
    critical.sort_by_key(|b| std::cmp::Reverse(b.file_count));
    shared.sort_by_key(|b| std::cmp::Reverse(b.file_count));
    minor.sort_by_key(|b| std::cmp::Reverse(b.file_count));

    (critical, shared, minor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_test_tx(conn: &rusqlite::Connection, id: i64, action: &str) -> i64 {
        conn.execute(
            "INSERT INTO transactions (id, action, timestamp, user, status, packages) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![id, action, "2024-01-01T00:00:00Z", "test-user", "completed", "test-pkg"],
        ).unwrap();
        id
    }

    fn open_in_memory() -> SpmResult<rusqlite::Connection> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        crate::db::init_schema(&conn)?;
        crate::db::conflict::init_conflict_schema(&conn)?;
        Ok(conn)
    }

    fn make_conf(pkg: &str, files: Vec<&str>) -> (String, Vec<String>) {
        (pkg.to_string(), files.into_iter().map(String::from).collect())
    }

    fn map_from_pairs(pairs: Vec<(String, Vec<String>)>) -> HashMap<String, Vec<String>> {
        pairs.into_iter().collect()
    }

    #[test]
    fn test_classify_empty() {
        let (c, s, m) = classify_conflicts(&HashMap::new());
        assert!(c.is_empty() && s.is_empty() && m.is_empty());
    }

    #[test]
    fn test_classify_minor() {
        let map = map_from_pairs(vec![
            make_conf("pkg-a", vec!["/usr/share/doc/readme", "/etc/config"]),
        ]);
        let (c, s, m) = classify_conflicts(&map);
        assert!(c.is_empty());
        assert!(s.is_empty());
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].package, "pkg-a");
        assert_eq!(m[0].file_count, 2);
    }

    #[test]
    fn test_classify_shared() {
        let map = map_from_pairs(vec![
            make_conf("pkg-b", vec!["/usr/share/a", "/usr/share/b", "/usr/share/c", "/usr/share/d"]),
        ]);
        let (c, s, m) = classify_conflicts(&map);
        assert!(c.is_empty());
        assert_eq!(s.len(), 1);
        assert_eq!(m.len(), 0);
        assert_eq!(s[0].package, "pkg-b");
        assert_eq!(s[0].file_count, 4);
    }

    #[test]
    fn test_classify_critical_bin_overlap() {
        let map = map_from_pairs(vec![
            make_conf("pkg-c", vec![
                "/usr/bin/prog", "/usr/bin/other", "/usr/share/x", "/usr/share/y",
                "/usr/share/z", "/etc/cfg", "/var/log/x",
            ]),
        ]);
        let (c, _s, _m) = classify_conflicts(&map);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].package, "pkg-c");
        assert!(c[0].reason.contains("bin/lib"));
    }

    #[test]
    fn test_classify_critical_lib_overlap() {
        let map = map_from_pairs(vec![
            make_conf("pkg-d", vec![
                "/usr/lib/libfoo.so", "/usr/lib/libbar.so", "/usr/share/a",
                "/usr/share/b", "/usr/share/c", "/etc/x", "/var/y",
            ]),
        ]);
        let (c, s, m) = classify_conflicts(&map);
        assert_eq!(c.len(), 1);
        assert_eq!(s.len(), 0);
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn test_classify_sorts_by_file_count() {
        let map = map_from_pairs(vec![
            make_conf("small", vec!["/usr/share/a"]),
            make_conf("medium", vec!["/usr/share/a", "/usr/share/b"]),
            make_conf("large", vec!["/usr/share/a", "/usr/share/b", "/usr/share/c"]),
        ]);
        let (c, s, m) = classify_conflicts(&map);
        assert!(c.is_empty());
        assert!(s.is_empty());
        assert_eq!(m.len(), 3);
        // Sorted descending by file_count
        assert!(m[0].file_count >= m[1].file_count);
        assert!(m[1].file_count >= m[2].file_count);
    }

    #[test]
    fn test_detect_file_conflicts_empty_db() {
        let conn = open_in_memory().unwrap();
        let new_files: HashSet<String> = ["/usr/bin/foo", "/usr/lib/libfoo.so"]
            .iter().map(|s| s.to_string()).collect();
        let result = detect_file_conflicts(&conn, &new_files, &[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_detect_file_conflicts_with_match() {
        let conn = open_in_memory().unwrap();
        let tx_id = insert_test_tx(&conn, 1, "install");
        conn.execute("INSERT INTO installed_packages (name, version, format, store_hash, install_type, install_date, source_repo) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params!["existing-pkg", "1.0", "Sam", "hash1", "user", "2024-01-01", "test"],
        ).unwrap();
        conn.execute("INSERT INTO files (transaction_id, package, format, filepath, hash, action) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![tx_id, "existing-pkg", "Sam", "/usr/bin/foo", "abc", "install"],
        ).unwrap();

        let new_files: HashSet<String> = ["/usr/bin/foo", "/usr/lib/libbar.so"]
            .iter().map(|s| s.to_string()).collect();
        let result = detect_file_conflicts(&conn, &new_files, &[]).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("existing-pkg"));
        assert_eq!(result["existing-pkg"].len(), 1);
        assert_eq!(result["existing-pkg"][0], "/usr/bin/foo");
    }

    #[test]
    fn test_detect_file_conflicts_exclude_package() {
        let conn = open_in_memory().unwrap();
        let tx_id = insert_test_tx(&conn, 2, "install");
        conn.execute("INSERT INTO installed_packages (name, version, format, store_hash, install_type, install_date, source_repo) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params!["to-replace", "1.0", "Sam", "hash2", "system", "2024-01-01", "test"],
        ).unwrap();
        conn.execute("INSERT INTO files (transaction_id, package, format, filepath, hash, action) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![tx_id, "to-replace", "Sam", "/usr/bin/foo", "abc", "install"],
        ).unwrap();

        let new_files: HashSet<String> = ["/usr/bin/foo"].iter().map(|s| s.to_string()).collect();
        let result = detect_file_conflicts(&conn, &new_files, &["to-replace".to_string()]).unwrap();
        assert!(result.is_empty(), "should exclude package being replaced");
    }

    #[test]
    fn test_conflict_severity_partial_eq() {
        assert_ne!(ConflictSeverity::Critical, ConflictSeverity::Shared);
        assert_ne!(ConflictSeverity::Shared, ConflictSeverity::Minor);
        assert_eq!(ConflictSeverity::Critical, ConflictSeverity::Critical);
    }

    #[test]
    fn test_conflict_log_entry_roundtrip() {
        let conn = open_in_memory().unwrap();
        insert_test_tx(&conn, 1, "install");

        let entry = ConflictLogEntry {
            transaction_id: 1,
            package_a: "old-pkg".into(),
            package_b: "new-pkg".into(),
            file_path: "/usr/bin/foo".into(),
            action: "replaced".into(),
            timestamp: "2024-01-01T00:00:00Z".into(),
        };
        let id = record_conflict(&conn, &entry).unwrap();
        assert!(id > 0);

        let entries = get_conflicts_by_transaction(&conn, 1).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].package_a, "old-pkg");
        assert_eq!(entries[0].file_path, "/usr/bin/foo");
        assert_eq!(entries[0].action, "replaced");
    }

    #[test]
    fn test_record_conflicts_batch() {
        let conn = open_in_memory().unwrap();
        insert_test_tx(&conn, 2, "install");

        let mut conflicts: HashMap<String, Vec<String>> = HashMap::new();
        conflicts.insert("old-pkg".into(), vec!["/usr/bin/foo".into(), "/usr/lib/libbar.so".into()]);
        record_conflicts_batch(&conn, 2, &conflicts, "new-pkg").unwrap();

        let entries = get_conflicts_for_package(&conn, "old-pkg").unwrap();
        assert_eq!(entries.len(), 2);
    }
}
