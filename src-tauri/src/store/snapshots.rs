//! Organizer verdicts on whether a memory is a point-in-time status snapshot.
//! An archived memory with a `snapshot` verdict is a tombstone: new writes that
//! closely match it are refused so the snapshot cannot come back.

use std::collections::HashMap;

use rusqlite::{params, Connection};

use super::memories::{self, Memory};
use super::with_conn;
use crate::services::dedup;

pub const SNAPSHOT: &str = "snapshot";
pub const KNOWLEDGE: &str = "knowledge";
pub const KEPT: &str = "kept";

pub const TOMBSTONE_SIMILARITY: f64 = 0.7;
const TOMBSTONE_CANDIDATES: i64 = 25;

fn ensure_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS snapshot_reviews (
            memory_id TEXT PRIMARY KEY,
            verdict TEXT NOT NULL,
            checked_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| format!("snapshot_reviews table: {}", e))
}

pub fn set_verdict(memory_id: &str, verdict: &str) -> Result<(), String> {
    with_conn(|conn| set_verdict_with_conn(conn, memory_id, verdict))
}

pub fn set_verdict_with_conn(conn: &Connection, memory_id: &str, verdict: &str) -> Result<(), String> {
    ensure_table(conn)?;
    conn.execute(
        "INSERT INTO snapshot_reviews (memory_id, verdict, checked_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(memory_id) DO UPDATE SET verdict = excluded.verdict, checked_at = excluded.checked_at",
        params![memory_id, verdict, chrono::Utc::now().timestamp()],
    )
    .map_err(|e| format!("set snapshot verdict: {}", e))?;
    Ok(())
}

pub fn verdicts() -> Result<HashMap<String, String>, String> {
    with_conn(verdicts_with_conn)
}

pub fn verdicts_with_conn(conn: &Connection) -> Result<HashMap<String, String>, String> {
    ensure_table(conn)?;
    let mut stmt = conn
        .prepare("SELECT memory_id, verdict FROM snapshot_reviews")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

pub fn is_snapshot(memory_id: &str) -> Result<bool, String> {
    Ok(verdicts()?.get(memory_id).map(String::as_str) == Some(SNAPSHOT))
}

pub fn tombstone_for(title: &str, description: &str, content: &str) -> Result<Option<Memory>, String> {
    with_conn(|conn| tombstone_for_with_conn(conn, title, description, content))
}

/// The archived snapshot this text would resurrect, if any. Scope is ignored:
/// a snapshot re-saved under another project is still the same snapshot.
pub fn tombstone_for_with_conn(
    conn: &Connection,
    title: &str,
    description: &str,
    content: &str,
) -> Result<Option<Memory>, String> {
    ensure_table(conn)?;
    let probe = format!("{title} {description} {content}");
    let query = memories::sanitize_fts_query(&probe);
    if query.is_empty() {
        return Ok(None);
    }
    let mut stmt = conn
        .prepare(
            r#"SELECT m.* FROM memories_fts
               JOIN memories m ON m.rowid = memories_fts.rowid
               JOIN snapshot_reviews s ON s.memory_id = m.id
               WHERE memories_fts MATCH ?1 AND m.archived_at IS NOT NULL AND s.verdict = ?2
               ORDER BY bm25(memories_fts)
               LIMIT ?3"#,
        )
        .map_err(|e| format!("prepare tombstone lookup: {}", e))?;
    let candidates = stmt
        .query_map(params![query, SNAPSHOT, TOMBSTONE_CANDIDATES], memories::row_to_memory)
        .map_err(|e| format!("tombstone lookup: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(candidates
        .into_iter()
        .map(|m| {
            let score = dedup::lexical_similarity_text(&probe, &dedup::embed_text(&m));
            (m, score)
        })
        .filter(|(_, score)| *score + f64::EPSILON >= TOMBSTONE_SIMILARITY)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(m, _)| m))
}
