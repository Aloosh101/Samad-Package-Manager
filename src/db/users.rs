use rusqlite::Connection;

use crate::error::SpmResult;
use crate::types::{PackageFormat, UserInstall};

pub fn record_user_install(conn: &Connection, user_id: u32, name: &str, format: &PackageFormat, hash: &str) -> SpmResult<()> {
    let fmt_str = serde_json::to_string(format)?;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR REPLACE INTO user_installs (user_id, package_name, package_format, package_hash, installed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![user_id, name, fmt_str, hash, now],
    )?;
    Ok(())
}

pub fn remove_user_install(conn: &Connection, user_id: u32, name: &str, format: &PackageFormat) -> SpmResult<()> {
    let fmt_str = serde_json::to_string(format)?;
    conn.execute(
        "DELETE FROM user_installs WHERE user_id = ?1 AND package_name = ?2 AND package_format = ?3",
        rusqlite::params![user_id, name, fmt_str],
    )?;
    Ok(())
}

pub fn is_installed_for_user(conn: &Connection, user_id: u32, name: &str) -> SpmResult<bool> {
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM user_installs WHERE user_id = ?1 AND package_name = ?2"
    )?;
    let count: i64 = stmt.query_row(rusqlite::params![user_id, name], |row| row.get(0))?;
    Ok(count > 0)
}

pub fn list_user_installs(conn: &Connection, user_id: u32) -> SpmResult<Vec<UserInstall>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, package_name, package_format, package_hash, installed_at \
         FROM user_installs WHERE user_id = ?1 ORDER BY package_name",
    )?;
    let rows = stmt.query_map(rusqlite::params![user_id], |row| {
        Ok(UserInstall {
            user_id: row.get(0)?,
            package_name: row.get(1)?,
            package_format: serde_json::from_str(&row.get::<_, String>(2)?)
                .unwrap_or(PackageFormat::Sam),
            package_hash: row.get(3)?,
            installed_at: row.get(4)?,
        })
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

pub fn count_users_for_package_hash(conn: &Connection, hash: &str) -> SpmResult<u32> {
    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM user_installs WHERE package_hash = ?1",
        rusqlite::params![hash],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn list_all_user_installs(conn: &Connection) -> SpmResult<Vec<UserInstall>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, package_name, package_format, package_hash, installed_at \
         FROM user_installs ORDER BY user_id, package_name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(UserInstall {
            user_id: row.get(0)?,
            package_name: row.get(1)?,
            package_format: serde_json::from_str(&row.get::<_, String>(2)?)
                .unwrap_or(PackageFormat::Sam),
            package_hash: row.get(3)?,
            installed_at: row.get(4)?,
        })
    })?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}
