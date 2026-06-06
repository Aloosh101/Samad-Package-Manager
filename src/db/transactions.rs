use rusqlite::Connection;

use crate::error::SpmResult;
use crate::types::{Transaction, TransactionAction, TransactionStatus};

pub fn record_transaction(conn: &Connection, tx: &Transaction) -> SpmResult<i64> {
    conn.execute(
        "INSERT INTO transactions (action, timestamp, user, status, packages, snapshot_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            serde_json::to_string(&tx.action)?,
            tx.timestamp,
            tx.user,
            serde_json::to_string(&tx.status)?,
            serde_json::to_string(&tx.packages)?,
            tx.snapshot_id,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_transaction(conn: &Connection, id: i64) -> SpmResult<Option<Transaction>> {
    let mut stmt = conn.prepare(
        "SELECT id, action, timestamp, user, status, packages, snapshot_id \
         FROM transactions WHERE id = ?1",
    )?;
    let mut rows = stmt.query(rusqlite::params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(Transaction {
            id: Some(row.get(0)?),
            action: serde_json::from_str(&row.get::<_, String>(1)?)?,
            timestamp: row.get(2)?,
            user: row.get(3)?,
            status: serde_json::from_str(&row.get::<_, String>(4)?)?,
            packages: serde_json::from_str(&row.get::<_, String>(5)?)?,
            snapshot_id: row.get(6)?,
        })),
        None => Ok(None),
    }
}

pub fn list_transactions(conn: &Connection) -> SpmResult<Vec<Transaction>> {
    let mut stmt = conn.prepare(
        "SELECT id, action, timestamp, user, status, packages, snapshot_id \
         FROM transactions ORDER BY id DESC LIMIT 50",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Transaction {
            id: Some(row.get(0)?),
            action: serde_json::from_str(&row.get::<_, String>(1)?).unwrap_or(TransactionAction::Install),
            timestamp: row.get(2)?,
            user: row.get(3)?,
            status: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or(TransactionStatus::Completed),
            packages: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
            snapshot_id: row.get(6)?,
        })
    })?;
    let mut txs = Vec::new();
    for row in rows {
        txs.push(row?);
    }
    Ok(txs)
}

pub fn update_transaction_status(conn: &Connection, id: i64, status: &TransactionStatus) -> SpmResult<()> {
    conn.execute(
        "UPDATE transactions SET status = ?1 WHERE id = ?2",
        rusqlite::params![serde_json::to_string(status)?, id],
    )?;
    Ok(())
}
