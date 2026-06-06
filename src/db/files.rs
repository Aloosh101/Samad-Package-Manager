use rusqlite::Connection;

use crate::error::SpmResult;
use crate::types::{FileAction, FileRecord, PackageFormat, PackageId};

fn row_to_file_record(row: &rusqlite::Row) -> rusqlite::Result<FileRecord> {
    Ok(FileRecord {
        id: Some(row.get(0)?),
        transaction_id: row.get(1)?,
        package: row.get(2)?,
        format: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or(PackageFormat::Deb),
        filepath: row.get(4)?,
        hash: row.get(5)?,
        action: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or(FileAction::Created),
    })
}

pub fn record_files(conn: &Connection, files: &[FileRecord]) -> SpmResult<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO files (transaction_id, package, format, filepath, hash, action) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for f in files {
        stmt.execute(rusqlite::params![
            f.transaction_id,
            f.package,
            serde_json::to_string(&f.format)?,
            f.filepath,
            f.hash,
            serde_json::to_string(&f.action)?,
        ])?;
    }
    Ok(())
}

pub fn get_files_by_package(conn: &Connection, pkg: &str) -> SpmResult<Vec<FileRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, transaction_id, package, format, filepath, hash, action \
         FROM files WHERE package = ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![pkg], row_to_file_record)?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

pub fn get_files_by_package_id(conn: &Connection, pid: &PackageId) -> SpmResult<Vec<FileRecord>> {
    let fmt_str = serde_json::to_string(&pid.format)?;
    let mut stmt = conn.prepare(
        "SELECT id, transaction_id, package, format, filepath, hash, action \
         FROM files WHERE package = ?1 AND format = ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![pid.name, fmt_str], row_to_file_record)?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

pub fn get_packages_for_file(conn: &Connection, filepath: &str) -> SpmResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT package FROM files WHERE filepath = ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![filepath], |row| row.get::<_, String>(0))?;
    let mut packages = Vec::new();
    for row in rows {
        packages.push(row?);
    }
    Ok(packages)
}

pub fn get_files_by_packages_batch(conn: &Connection, names: &[String]) -> SpmResult<Vec<FileRecord>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = names.iter().enumerate().map(|(i, _)| format!("?{}", i + 1)).collect();
    let sql = format!(
        "SELECT id, transaction_id, package, format, filepath, hash, action \
         FROM files WHERE package IN ({})",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = names.iter().map(|n| n as &dyn rusqlite::types::ToSql).collect();
    let rows = stmt.query_map(params.as_slice(), row_to_file_record)?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

pub fn get_all_files(conn: &Connection) -> SpmResult<Vec<FileRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, transaction_id, package, format, filepath, hash, action FROM files",
    )?;
    let rows = stmt.query_map([], row_to_file_record)?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

pub fn get_files_for_transaction(conn: &Connection, tx_id: i64) -> SpmResult<Vec<FileRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, transaction_id, package, format, filepath, hash, action \
         FROM files WHERE transaction_id = ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![tx_id], row_to_file_record)?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}
