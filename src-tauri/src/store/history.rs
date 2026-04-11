//! Simple history log for destructive organizer operations.
//! Records enough state to reverse merges and reclassifications.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::with_conn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub action: String,
    pub timestamp: i64,
    pub snapshot: String, // JSON blob
}

pub fn ensure_table() -> Result<(), String> {
    with_conn(|conn| {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                snapshot TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_history_timestamp ON history(timestamp DESC);
            "#,
        )
        .map_err(|e| format!("history table: {}", e))
    })
}

pub fn record(action: &str, snapshot: serde_json::Value) -> Result<i64, String> {
    ensure_table()?;
    with_conn(|conn| {
        let now = chrono::Utc::now().timestamp();
        let snap_str = snapshot.to_string();
        conn.execute(
            "INSERT INTO history (action, timestamp, snapshot) VALUES (?1, ?2, ?3)",
            params![action, now, snap_str],
        )
        .map_err(|e| format!("insert history: {}", e))?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn list_recent(limit: i64) -> Result<Vec<HistoryEntry>, String> {
    ensure_table()?;
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT id, action, timestamp, snapshot FROM history ORDER BY timestamp DESC LIMIT ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(HistoryEntry {
                    id: r.get(0)?,
                    action: r.get(1)?,
                    timestamp: r.get(2)?,
                    snapshot: r.get(3)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

pub fn get_most_recent() -> Result<Option<HistoryEntry>, String> {
    let recent = list_recent(1)?;
    Ok(recent.into_iter().next())
}

pub fn delete_entry(id: i64) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute("DELETE FROM history WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}
