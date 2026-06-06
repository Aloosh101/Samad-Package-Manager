use std::collections::HashMap;

use rusqlite::Connection;
use rusqlite::OptionalExtension;

use crate::error::SpmResult;
use crate::types::{InstallOrigin, InstallType, InstalledPackage, PackageFormat, PackageId};

fn origin_from_str(s: Option<&str>) -> InstallOrigin {
    match s {
        Some("foreign") => InstallOrigin::Foreign,
        _ => InstallOrigin::Spm,
    }
}

fn row_to_installed_package(row: &rusqlite::Row) -> rusqlite::Result<InstalledPackage> {
    Ok(InstalledPackage {
        name: row.get(0)?,
        version: row.get(1)?,
        format: serde_json::from_str(&row.get::<_, String>(2)?)
            .unwrap_or(PackageFormat::Sam),
        install_type: serde_json::from_str(&row.get::<_, String>(3)?)
            .unwrap_or(InstallType::Native),
        manifest: row.get(4)?,
        install_date: row.get(5)?,
        source_repo: row.get(6)?,
        store_hash: row.get(7)?,
        origin: origin_from_str(row.get::<_, Option<String>>(8).ok().flatten().as_deref()),
    })
}

fn query_package_columns() -> &'static str {
    "name, version, format, install_type, manifest, install_date, source_repo, store_hash, origin"
}

pub fn get_installed_package(conn: &Connection, name: &str) -> SpmResult<Option<InstalledPackage>> {
    let mut stmt = conn.prepare(
        &format!("SELECT {} FROM installed_packages WHERE name = ?1 ORDER BY format ASC LIMIT 1", query_package_columns()),
    )?;
    let mut rows = stmt.query(rusqlite::params![name])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_installed_package(row)?)),
        None => Ok(None),
    }
}

pub fn get_installed_package_by_id(conn: &Connection, pid: &PackageId) -> SpmResult<Option<InstalledPackage>> {
    let mut stmt = conn.prepare(
        &format!("SELECT {} FROM installed_packages WHERE name = ?1 AND format = ?2", query_package_columns()),
    )?;
    let fmt_str = serde_json::to_string(&pid.format)?;
    let mut rows = stmt.query(rusqlite::params![pid.name, fmt_str])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_installed_package(row)?)),
        None => Ok(None),
    }
}

pub fn list_installed_packages_by_format(conn: &Connection, format: &PackageFormat) -> SpmResult<Vec<InstalledPackage>> {
    let fmt_str = serde_json::to_string(format)?;
    let mut stmt = conn.prepare(
        &format!("SELECT {} FROM installed_packages WHERE format = ?1 ORDER BY name", query_package_columns()),
    )?;
    let rows = stmt.query_map(rusqlite::params![fmt_str], row_to_installed_package)?;
    let mut packages = Vec::new();
    for row in rows {
        packages.push(row?);
    }
    Ok(packages)
}

pub fn list_installed_packages(conn: &Connection) -> SpmResult<Vec<InstalledPackage>> {
    let mut stmt = conn.prepare(
        &format!("SELECT {} FROM installed_packages ORDER BY name", query_package_columns()),
    )?;
    let rows = stmt.query_map([], row_to_installed_package)?;
    let mut packages = Vec::new();
    for row in rows {
        packages.push(row?);
    }
    Ok(packages)
}

pub fn add_installed_package(conn: &Connection, pkg: &InstalledPackage) -> SpmResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO installed_packages (name, version, format, install_type, manifest, install_date, source_repo, store_hash, origin) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            pkg.name,
            pkg.version,
            serde_json::to_string(&pkg.format)?,
            serde_json::to_string(&pkg.install_type)?,
            pkg.manifest,
            pkg.install_date,
            pkg.source_repo,
            pkg.store_hash,
            pkg.origin.to_string(),
        ],
    )?;
    Ok(())
}

pub fn get_store_hash(conn: &Connection, name: &str, _format: &PackageFormat) -> SpmResult<Option<String>> {
    conn.query_row(
        "SELECT store_hash FROM installed_packages WHERE name = ?1",
        rusqlite::params![name],
        |row| row.get(0),
    ).optional().map(|o| o.flatten()).map_err(Into::into)
}

pub fn remove_installed_package(conn: &Connection, name: &str) -> SpmResult<()> {
    conn.execute(
        "DELETE FROM installed_packages WHERE name = ?1",
        rusqlite::params![name],
    )?;
    Ok(())
}

pub fn remove_installed_package_by_id(conn: &Connection, pid: &PackageId) -> SpmResult<()> {
    let fmt_str = serde_json::to_string(&pid.format)?;
    conn.execute(
        "DELETE FROM files WHERE package = ?1",
        rusqlite::params![pid.name],
    )?;
    conn.execute(
        "DELETE FROM installed_packages WHERE name = ?1 AND format = ?2",
        rusqlite::params![pid.name, fmt_str],
    )?;
    Ok(())
}

pub fn is_installed(conn: &Connection, name: &str) -> bool {
    get_installed_package(conn, name).ok().flatten().is_some()
}

pub fn is_installed_by_id(conn: &Connection, pid: &PackageId) -> bool {
    get_installed_package_by_id(conn, pid).ok().flatten().is_some()
}

pub fn get_all_installed_package_names(conn: &Connection) -> SpmResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT name FROM installed_packages")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

pub fn get_all_installed_store_hashes(conn: &Connection) -> SpmResult<HashMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT name, store_hash FROM installed_packages WHERE store_hash IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (name, hash) = row?;
        map.insert(name, hash);
    }
    Ok(map)
}

pub fn get_installed_packages_batch(conn: &Connection, names: &[String]) -> SpmResult<Vec<(String, InstalledPackage)>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = names.iter().enumerate().map(|(i, _)| format!("?{}", i + 1)).collect();
    let sql = format!(
        "SELECT {} FROM installed_packages WHERE name IN ({})",
        query_package_columns(),
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = names.iter().map(|n| n as &dyn rusqlite::types::ToSql).collect();
    let rows = stmt.query_map(params.as_slice(), |row| {
        let name: String = row.get(0)?;
        let pkg = row_to_installed_package(row)?;
        Ok((name, pkg))
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

pub fn get_foreign_packages(conn: &Connection) -> SpmResult<Vec<InstalledPackage>> {
    let mut stmt = conn.prepare(
        &format!("SELECT {} FROM installed_packages WHERE origin = 'foreign' ORDER BY name", query_package_columns()),
    )?;
    let rows = stmt.query_map([], row_to_installed_package)?;
    let mut packages = Vec::new();
    for row in rows {
        packages.push(row?);
    }
    Ok(packages)
}

pub fn count_packages_by_origin(conn: &Connection) -> SpmResult<(usize, usize)> {
    let spm_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM installed_packages WHERE origin = 'spm'",
        [], |row| row.get(0),
    )?;
    let foreign_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM installed_packages WHERE origin = 'foreign'",
        [], |row| row.get(0),
    )?;
    Ok((spm_count as usize, foreign_count as usize))
}

pub fn get_installed_package_names_by_origin(conn: &Connection, origin: &InstallOrigin) -> SpmResult<Vec<String>> {
    let origin_str = origin.to_string();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT name FROM installed_packages WHERE origin = ?1 ORDER BY name",
    )?;
    let rows = stmt.query_map(rusqlite::params![origin_str], |row| row.get::<_, String>(0))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}
