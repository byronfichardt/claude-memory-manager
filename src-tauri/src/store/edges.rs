use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};

use super::with_conn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEdge {
    pub id: i64,
    pub source_id: String,
    pub target_id: String,
    pub edge_type: String,
    pub weight: f64,
    pub source_origin: String,
    pub created_at: i64,
    pub updated_at: i64,
}

fn row_to_edge(row: &Row) -> rusqlite::Result<MemoryEdge> {
    Ok(MemoryEdge {
        id: row.get("id")?,
        source_id: row.get("source_id")?,
        target_id: row.get("target_id")?,
        edge_type: row.get("edge_type")?,
        weight: row.get("weight")?,
        source_origin: row.get("source_origin")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Insert or update an edge. If (source_id, target_id, edge_type) already exists,
/// updates weight to the max of existing and new, and bumps updated_at.
pub fn insert(
    source_id: &str,
    target_id: &str,
    edge_type: &str,
    weight: f64,
    source_origin: &str,
) -> Result<MemoryEdge, String> {
    with_conn(|conn| insert_with_conn(conn, source_id, target_id, edge_type, weight, source_origin))
}

pub fn insert_with_conn(
    conn: &Connection,
    source_id: &str,
    target_id: &str,
    edge_type: &str,
    weight: f64,
    source_origin: &str,
) -> Result<MemoryEdge, String> {
    let now = now_ts();
    conn.execute(
        r#"INSERT INTO memory_edges (source_id, target_id, edge_type, weight, source_origin, created_at, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
           ON CONFLICT(source_id, target_id, edge_type) DO UPDATE SET
               weight = MAX(memory_edges.weight, excluded.weight),
               updated_at = excluded.updated_at"#,
        params![source_id, target_id, edge_type, weight, source_origin, now, now],
    )
    .map_err(|e| format!("insert edge: {}", e))?;

    // Fetch the inserted/updated edge
    let edge = conn
        .query_row(
            "SELECT * FROM memory_edges WHERE source_id = ?1 AND target_id = ?2 AND edge_type = ?3",
            params![source_id, target_id, edge_type],
            row_to_edge,
        )
        .map_err(|e| format!("fetch inserted edge: {}", e))?;

    Ok(edge)
}

/// Strengthen an existing edge by adding delta to its weight (capped at 1.0).
/// No-op if the edge doesn't exist.
pub fn strengthen(source_id: &str, target_id: &str, edge_type: &str, delta: f64) -> Result<(), String> {
    with_conn(|conn| strengthen_with_conn(conn, source_id, target_id, edge_type, delta))
}

pub fn strengthen_with_conn(
    conn: &Connection,
    source_id: &str,
    target_id: &str,
    edge_type: &str,
    delta: f64,
) -> Result<(), String> {
    let now = now_ts();
    conn.execute(
        r#"UPDATE memory_edges
           SET weight = MIN(1.0, weight + ?4), updated_at = ?5
           WHERE source_id = ?1 AND target_id = ?2 AND edge_type = ?3"#,
        params![source_id, target_id, edge_type, delta, now],
    )
    .map_err(|e| format!("strengthen edge: {}", e))?;
    Ok(())
}

/// Get all direct neighbors (1-hop) of a memory.
pub fn get_neighbors(memory_id: &str) -> Result<Vec<MemoryEdge>, String> {
    with_conn(|conn| get_neighbors_with_conn(conn, memory_id))
}

pub fn get_neighbors_with_conn(conn: &Connection, memory_id: &str) -> Result<Vec<MemoryEdge>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT * FROM memory_edges WHERE source_id = ?1 OR target_id = ?1 ORDER BY weight DESC",
        )
        .map_err(|e| format!("prepare get_neighbors: {}", e))?;

    let rows = stmt
        .query_map(params![memory_id], row_to_edge)
        .map_err(|e| format!("query get_neighbors: {}", e))?;

    let mut edges = Vec::new();
    for r in rows {
        edges.push(r.map_err(|e| e.to_string())?);
    }
    Ok(edges)
}

/// Get all edges connected to any of the given memory IDs (1-hop batch query).
/// Used by the hook for efficient graph expansion.
pub fn get_neighbors_batch(ids: &[&str]) -> Result<Vec<MemoryEdge>, String> {
    with_conn(|conn| get_neighbors_batch_with_conn(conn, ids))
}

pub fn get_neighbors_batch_with_conn(
    conn: &Connection,
    ids: &[&str],
) -> Result<Vec<MemoryEdge>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    // UNION over two single-column IN clauses lets SQLite use
    // idx_edges_source and idx_edges_target as index seeks (an OR of two INs
    // collapses to a scan) and each id binds once per branch.
    let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{}", i)).collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        r#"SELECT * FROM memory_edges WHERE source_id IN ({in_clause})
           UNION
           SELECT * FROM memory_edges WHERE target_id IN ({in_clause})
           ORDER BY weight DESC"#
    );

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare get_neighbors_batch: {}", e))?;

    let params_iter = ids.iter().copied().chain(ids.iter().copied());
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params_iter), row_to_edge)
        .map_err(|e| format!("query get_neighbors_batch: {}", e))?;

    let mut edges = Vec::new();
    for r in rows {
        edges.push(r.map_err(|e| e.to_string())?);
    }
    Ok(edges)
}

/// Create or strengthen co-access edges between all (i, j) pairs from `ids` in
/// a single transaction with one prepared statement. Replaces the old
/// N²-roundtrip pattern the hook ran on every prompt: per pair, one INSERT
/// plus one UPDATE — 20 sequential writes for 5 hits.
///
/// The upsert handles both "new edge" and "strengthen existing": the initial
/// weight is used on first insert; on conflict the existing weight is bumped
/// by `delta` (capped at 1.0). Collapses to 1 transaction + N² stmt executions
/// reusing a cached prepared statement.
pub fn strengthen_co_access_batch(
    conn: &Connection,
    ids: &[&str],
    initial_weight: f64,
    delta: f64,
) -> Result<(), String> {
    if ids.len() < 2 {
        return Ok(());
    }

    let now = now_ts();
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| format!("begin co-access tx: {}", e))?;

    let result = (|| -> Result<(), String> {
        let mut stmt = conn
            .prepare(
                r#"INSERT INTO memory_edges (source_id, target_id, edge_type, weight, source_origin, created_at, updated_at)
                   VALUES (?1, ?2, 'relates-to', ?3, 'co_access', ?4, ?4)
                   ON CONFLICT(source_id, target_id, edge_type) DO UPDATE SET
                       weight = MIN(1.0, weight + ?5),
                       updated_at = ?4"#,
            )
            .map_err(|e| format!("prepare co-access upsert: {}", e))?;

        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                stmt.execute(params![ids[i], ids[j], initial_weight, now, delta])
                    .map_err(|e| format!("co-access upsert: {}", e))?;
            }
        }
        Ok(())
    })();

    if result.is_ok() {
        conn.execute_batch("COMMIT")
            .map_err(|e| format!("commit co-access tx: {}", e))?;
    } else {
        let _ = conn.execute_batch("ROLLBACK");
    }
    result
}

/// Get neighbors up to N hops using a recursive CTE.
/// Used by the MCP tool for deeper traversal.
pub fn get_neighbors_deep(memory_id: &str, depth: u32) -> Result<Vec<MemoryEdge>, String> {
    if depth == 0 {
        return Ok(Vec::new());
    }
    if depth == 1 {
        return get_neighbors(memory_id);
    }

    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                r#"WITH RECURSIVE graph(memory_id, depth) AS (
                    VALUES (?1, 0)
                    UNION
                    SELECT CASE
                        WHEN e.source_id = g.memory_id THEN e.target_id
                        ELSE e.source_id
                    END,
                    g.depth + 1
                    FROM memory_edges e
                    JOIN graph g ON (e.source_id = g.memory_id OR e.target_id = g.memory_id)
                    WHERE g.depth < ?2
                )
                SELECT DISTINCT e.* FROM memory_edges e
                WHERE e.source_id IN (SELECT memory_id FROM graph)
                   OR e.target_id IN (SELECT memory_id FROM graph)
                ORDER BY e.weight DESC"#,
            )
            .map_err(|e| format!("prepare deep neighbors: {}", e))?;

        let rows = stmt
            .query_map(params![memory_id, depth], row_to_edge)
            .map_err(|e| format!("query deep neighbors: {}", e))?;

        let mut edges = Vec::new();
        for r in rows {
            edges.push(r.map_err(|e| e.to_string())?);
        }
        Ok(edges)
    })
}

/// List all edges for a specific memory (for UI display).
pub fn list_by_memory(memory_id: &str) -> Result<Vec<MemoryEdge>, String> {
    get_neighbors(memory_id)
}

/// Delete edges by their IDs (for undo support).
pub fn delete_by_ids(edge_ids: &[i64]) -> Result<(), String> {
    if edge_ids.is_empty() {
        return Ok(());
    }

    with_conn(|conn| {
        let placeholders: Vec<String> = (1..=edge_ids.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "DELETE FROM memory_edges WHERE id IN ({})",
            placeholders.join(", ")
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("prepare delete_by_ids: {}", e))?;

        let param_values: Vec<Box<dyn rusqlite::types::ToSql>> =
            edge_ids.iter().map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>).collect();
        let refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();

        stmt.execute(refs.as_slice())
            .map_err(|e| format!("delete_by_ids: {}", e))?;
        Ok(())
    })
}

/// Total edge count (for stats).
pub fn count() -> Result<i64, String> {
    with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM memory_edges", [], |r| r.get(0))
            .map_err(|e| e.to_string())
    })
}
