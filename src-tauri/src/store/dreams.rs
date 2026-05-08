use rusqlite::{params, Row};
use serde::{Deserialize, Serialize};

use super::with_conn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamProposal {
    pub id: String,
    pub proposal_type: String, // "new" | "stale"
    pub title: String,
    pub content: String,
    pub description: String,
    pub memory_type: String,
    pub reasoning: String,
    pub target_memory_id: Option<String>,
    pub status: String, // "pending" | "applied" | "dismissed"
    pub created_at: i64,
}

fn row_to_proposal(row: &Row) -> rusqlite::Result<DreamProposal> {
    Ok(DreamProposal {
        id: row.get("id")?,
        proposal_type: row.get("proposal_type")?,
        title: row.get("title")?,
        content: row.get("content")?,
        description: row.get("description")?,
        memory_type: row.get("memory_type")?,
        reasoning: row.get("reasoning")?,
        target_memory_id: row.get("target_memory_id")?,
        status: row.get("status")?,
        created_at: row.get("created_at")?,
    })
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn save_proposal(proposal: &DreamProposal) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            r#"INSERT OR REPLACE INTO dream_proposals
               (id, proposal_type, title, content, description, memory_type, reasoning, target_memory_id, status, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
            params![
                proposal.id,
                proposal.proposal_type,
                proposal.title,
                proposal.content,
                proposal.description,
                proposal.memory_type,
                proposal.reasoning,
                proposal.target_memory_id,
                proposal.status,
                proposal.created_at,
            ],
        )
        .map_err(|e| format!("save dream proposal: {}", e))?;
        Ok(())
    })
}

pub fn save_proposals(proposals: &[DreamProposal]) -> Result<(), String> {
    // Clear pending proposals before saving new batch — each dream run is a fresh slate
    clear_pending()?;
    for p in proposals {
        save_proposal(p)?;
    }
    Ok(())
}

pub fn list_pending() -> Result<Vec<DreamProposal>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT * FROM dream_proposals WHERE status = 'pending' ORDER BY proposal_type DESC, created_at ASC")
            .map_err(|e| format!("prepare list_pending: {}", e))?;
        let rows = stmt
            .query_map([], row_to_proposal)
            .map_err(|e| format!("query list_pending: {}", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

pub fn set_status(id: &str, status: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "UPDATE dream_proposals SET status = ?1 WHERE id = ?2",
            params![status, id],
        )
        .map_err(|e| format!("set dream status: {}", e))?;
        Ok(())
    })
}

pub fn get(id: &str) -> Result<Option<DreamProposal>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT * FROM dream_proposals WHERE id = ?1")
            .map_err(|e| format!("prepare get dream: {}", e))?;
        let mut rows = stmt
            .query_map(params![id], row_to_proposal)
            .map_err(|e| format!("query get dream: {}", e))?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(|e| e.to_string())?)),
            None => Ok(None),
        }
    })
}

pub fn pending_count() -> Result<i64, String> {
    with_conn(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM dream_proposals WHERE status = 'pending'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
    })
}

fn clear_pending() -> Result<(), String> {
    with_conn(|conn| {
        conn.execute("DELETE FROM dream_proposals WHERE status = 'pending'", [])
            .map_err(|e| format!("clear pending dreams: {}", e))?;
        Ok(())
    })
}

pub fn now() -> i64 {
    now_ts()
}
