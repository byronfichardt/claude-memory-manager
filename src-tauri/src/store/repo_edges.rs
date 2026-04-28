use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};

use super::with_conn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEdge {
    pub id: i64,
    pub source_repo: String,
    pub target_repo: String,
    pub relationship_type: String,
    pub evidence: String,
    pub weight: f64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoGraph {
    pub nodes: Vec<String>,
    pub edges: Vec<RepoEdge>,
}

fn row_to_edge(row: &Row) -> rusqlite::Result<RepoEdge> {
    Ok(RepoEdge {
        id: row.get("id")?,
        source_repo: row.get("source_repo")?,
        target_repo: row.get("target_repo")?,
        relationship_type: row.get("relationship_type")?,
        evidence: row.get("evidence")?,
        weight: row.get("weight")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Insert or update a repo edge. On conflict (same source, target, type),
/// appends the new evidence and bumps updated_at.
pub fn upsert(
    source_repo: &str,
    target_repo: &str,
    relationship_type: &str,
    evidence: &str,
) -> Result<RepoEdge, String> {
    with_conn(|conn| upsert_with_conn(conn, source_repo, target_repo, relationship_type, evidence))
}

pub fn upsert_with_conn(
    conn: &Connection,
    source_repo: &str,
    target_repo: &str,
    relationship_type: &str,
    evidence: &str,
) -> Result<RepoEdge, String> {
    let now = now_ts();

    // Append evidence if already exists, otherwise insert fresh.
    conn.execute(
        r#"INSERT INTO repo_edges (source_repo, target_repo, relationship_type, evidence, weight, created_at, updated_at)
           VALUES (?1, ?2, ?3, ?4, 1.0, ?5, ?5)
           ON CONFLICT(source_repo, target_repo, relationship_type) DO UPDATE SET
               evidence = CASE
                   WHEN evidence = '' THEN excluded.evidence
                   WHEN excluded.evidence = '' THEN evidence
                   WHEN instr(evidence, excluded.evidence) > 0 THEN evidence
                   ELSE evidence || '; ' || excluded.evidence
               END,
               updated_at = ?5"#,
        params![source_repo, target_repo, relationship_type, evidence, now],
    )
    .map_err(|e| format!("upsert repo_edge: {}", e))?;

    conn.query_row(
        "SELECT * FROM repo_edges WHERE source_repo = ?1 AND target_repo = ?2 AND relationship_type = ?3",
        params![source_repo, target_repo, relationship_type],
        row_to_edge,
    )
    .map_err(|e| format!("fetch upserted repo_edge: {}", e))
}

/// Get all repo edges, optionally filtered to those involving a specific repo.
pub fn list(filter_repo: Option<&str>) -> Result<Vec<RepoEdge>, String> {
    with_conn(|conn| {
        let edges = match filter_repo {
            None => {
                let mut stmt = conn
                    .prepare("SELECT * FROM repo_edges ORDER BY updated_at DESC")
                    .map_err(|e| format!("prepare list_repo_edges: {}", e))?;
                let rows = stmt
                    .query_map([], row_to_edge)
                    .map_err(|e| format!("query list_repo_edges: {}", e))?;
                collect(rows)?
            }
            Some(repo) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT * FROM repo_edges WHERE source_repo = ?1 OR target_repo = ?1 ORDER BY updated_at DESC",
                    )
                    .map_err(|e| format!("prepare list_repo_edges_filtered: {}", e))?;
                let rows = stmt
                    .query_map(params![repo], row_to_edge)
                    .map_err(|e| format!("query list_repo_edges_filtered: {}", e))?;
                collect(rows)?
            }
        };
        Ok(edges)
    })
}

/// Build the full repo graph: unique repo names as nodes + all edges.
pub fn full_graph() -> Result<RepoGraph, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT * FROM repo_edges ORDER BY updated_at DESC")
            .map_err(|e| format!("prepare full_graph: {}", e))?;
        let rows = stmt
            .query_map([], row_to_edge)
            .map_err(|e| format!("query full_graph: {}", e))?;
        let edges = collect(rows)?;

        let mut nodes: Vec<String> = Vec::new();
        for edge in &edges {
            if !nodes.contains(&edge.source_repo) {
                nodes.push(edge.source_repo.clone());
            }
            if !nodes.contains(&edge.target_repo) {
                nodes.push(edge.target_repo.clone());
            }
        }
        nodes.sort();

        Ok(RepoGraph { nodes, edges })
    })
}

/// Get repos that the current repo depends on (outgoing edges from source_repo).
pub fn dependencies_of(repo: &str) -> Result<Vec<RepoEdge>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT * FROM repo_edges WHERE source_repo = ?1 ORDER BY weight DESC")
            .map_err(|e| format!("prepare dependencies_of: {}", e))?;
        let rows = stmt
            .query_map(params![repo], row_to_edge)
            .map_err(|e| format!("query dependencies_of: {}", e))?;
        collect(rows)
    })
}

/// Get repos that call/depend on the given repo (incoming edges to target_repo).
pub fn dependents_of(repo: &str) -> Result<Vec<RepoEdge>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT * FROM repo_edges WHERE target_repo = ?1 ORDER BY weight DESC")
            .map_err(|e| format!("prepare dependents_of: {}", e))?;
        let rows = stmt
            .query_map(params![repo], row_to_edge)
            .map_err(|e| format!("query dependents_of: {}", e))?;
        collect(rows)
    })
}

pub fn delete(id: i64) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute("DELETE FROM repo_edges WHERE id = ?1", params![id])
            .map_err(|e| format!("delete repo_edge: {}", e))?;
        Ok(())
    })
}

pub fn count() -> Result<i64, String> {
    with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM repo_edges", [], |r| r.get(0))
            .map_err(|e| e.to_string())
    })
}

fn collect(
    rows: impl Iterator<Item = rusqlite::Result<RepoEdge>>,
) -> Result<Vec<RepoEdge>, String> {
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}
