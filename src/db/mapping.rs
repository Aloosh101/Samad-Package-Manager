use crate::error::SpmResult;
use crate::types::PackageFormat;

pub fn resolve_name_mapping(deb_name: &str) -> SpmResult<Option<(String, String)>> {
    let conn = super::get_connection()?;
    let mut stmt = conn.prepare(
        "SELECT rpm_name, lib_soname FROM name_mappings WHERE deb_name = ?1",
    )?;
    let mut rows = stmt.query(rusqlite::params![deb_name])?;
    match rows.next()? {
        Some(row) => Ok(Some((row.get(0)?, row.get(1)?))),
        None => Ok(None),
    }
}

pub fn resolve_rpm_to_deb(rpm_name: &str) -> SpmResult<Option<(String, String)>> {
    let conn = super::get_connection()?;
    let mut stmt = conn.prepare(
        "SELECT deb_name, lib_soname FROM name_mappings WHERE rpm_name = ?1",
    )?;
    let mut rows = stmt.query(rusqlite::params![rpm_name])?;
    match rows.next()? {
        Some(row) => Ok(Some((row.get(0)?, row.get(1)?))),
        None => Ok(None),
    }
}

pub fn get_format_priority() -> SpmResult<Vec<(PackageFormat, i64)>> {
    let conn = super::get_connection()?;
    let mut stmt = conn.prepare(
        "SELECT format, priority FROM format_priority ORDER BY priority ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        let fmt_str: String = row.get(0)?;
        let priority: i64 = row.get(1)?;
        Ok((fmt_str, priority))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (fmt_str, priority) = row?;
        let format: PackageFormat = serde_json::from_str(&fmt_str).unwrap_or(PackageFormat::Sam);
        result.push((format, priority));
    }
    Ok(result)
}
