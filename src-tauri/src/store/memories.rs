use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{edges, settings, topics, with_conn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub title: String,
    pub description: String,
    pub content: String,
    pub memory_type: Option<String>,
    pub topic: Option<String>,
    pub source: Option<String>,
    pub project: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub access_count: i64,
    /// When set, the memory is archived: kept in the store but excluded from
    /// recall (search + graph hydration). Set by prune-on-supersede; reversible.
    pub archived_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub description: String,
    pub snippet: String,
    pub topic: Option<String>,
    pub memory_type: Option<String>,
    pub project: Option<String>,
    pub score: f64,
    #[serde(default)]
    pub access_count: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMemory {
    pub title: String,
    pub description: String,
    pub content: String,
    pub memory_type: Option<String>,
    pub topic: Option<String>,
    pub source: Option<String>,
    pub project: Option<String>,
}

fn row_to_memory(row: &Row) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: row.get("id")?,
        title: row.get("title")?,
        description: row.get("description")?,
        content: row.get("content")?,
        memory_type: row.get("memory_type")?,
        topic: row.get("topic")?,
        source: row.get("source")?,
        project: row.get("project")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        access_count: row.get("access_count")?,
        archived_at: row.get("archived_at")?,
    })
}

fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Insert a new memory. Returns the created memory.
/// If an existing memory has the same content hash, returns the existing one (idempotent).
pub fn insert(new: NewMemory) -> Result<Memory, String> {
    with_conn(|conn| insert_with_conn(conn, new))
}

/// Connection-owning variant of `insert` — used by the hook, which runs a raw
/// SQLite connection rather than checking out from the r2d2 pool.
pub fn insert_with_conn(conn: &Connection, new: NewMemory) -> Result<Memory, String> {
    let hash = content_hash(&new.content);

    if let Some(existing) = find_by_hash(conn, &hash)? {
        return Ok(existing);
    }

    // Auto-create topic if provided — the FK constraint requires it to exist.
    if let Some(ref t) = new.topic {
        topics::ensure_with_conn(conn, t, None, None)?;
    }

    let id = uuid::Uuid::new_v4().to_string();
    let now = now_ts();

    conn.execute(
        r#"INSERT INTO memories
           (id, title, description, content, content_hash, memory_type, topic, source, project, created_at, updated_at, access_count)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0)"#,
        params![
            id,
            new.title,
            new.description,
            new.content,
            hash,
            new.memory_type,
            new.topic,
            new.source,
            new.project,
            now,
            now,
        ],
    )
    .map_err(|e| format!("insert: {}", e))?;

    let created = get_by_id(conn, &id)?.ok_or_else(|| "insert succeeded but row missing".to_string())?;
    crate::services::embeddings::queue_memory(
        &created.id,
        &format!("{} {}", created.title, created.content),
    );
    Ok(created)
}

fn find_by_hash(conn: &Connection, hash: &str) -> Result<Option<Memory>, String> {
    let mut stmt = conn
        .prepare("SELECT * FROM memories WHERE content_hash = ?1 LIMIT 1")
        .map_err(|e| format!("prepare find_by_hash: {}", e))?;

    let mut rows = stmt
        .query(params![hash])
        .map_err(|e| format!("query find_by_hash: {}", e))?;

    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        Ok(Some(row_to_memory(row).map_err(|e| e.to_string())?))
    } else {
        Ok(None)
    }
}

fn get_by_id(conn: &Connection, id: &str) -> Result<Option<Memory>, String> {
    let mut stmt = conn
        .prepare("SELECT * FROM memories WHERE id = ?1")
        .map_err(|e| format!("prepare get: {}", e))?;

    let mut rows = stmt.query(params![id]).map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        Ok(Some(row_to_memory(row).map_err(|e| e.to_string())?))
    } else {
        Ok(None)
    }
}

pub fn get(id: &str) -> Result<Option<Memory>, String> {
    with_conn(|conn| get_by_id(conn, id))
}

pub fn update(
    id: &str,
    title: &str,
    description: &str,
    content: &str,
    topic: Option<&str>,
) -> Result<Memory, String> {
    with_conn(|conn| {
        let now = now_ts();
        let hash = content_hash(content);

        conn.execute(
            r#"UPDATE memories
               SET title = ?1, description = ?2, content = ?3, content_hash = ?4,
                   topic = ?5, updated_at = ?6
               WHERE id = ?7"#,
            params![title, description, content, hash, topic, now, id],
        )
        .map_err(|e| format!("update: {}", e))?;

        // Flag connected memories for staleness review
        flag_dependents_for_review(id);

        let updated = get_by_id(conn, id)?.ok_or_else(|| format!("memory {} not found after update", id))?;
        crate::services::embeddings::queue_memory(
            &updated.id,
            &format!("{} {}", updated.title, updated.content),
        );
        Ok(updated)
    })
}

/// Update only the project field on an existing memory (used by UI scope editor).
pub fn update_project(id: &str, project: Option<&str>) -> Result<Memory, String> {
    with_conn(|conn| {
        let now = now_ts();
        conn.execute(
            "UPDATE memories SET project = ?1, updated_at = ?2 WHERE id = ?3",
            params![project, now, id],
        )
        .map_err(|e| format!("update_project: {}", e))?;

        get_by_id(conn, id)?.ok_or_else(|| format!("memory {} not found after update_project", id))
    })
}

/// Archive a memory: exclude it from recall (search + graph hydration) while
/// keeping the row and its embedding in the store, so the action is fully
/// reversible via `unarchive`. Used by prune-on-supersede. No-op if already
/// archived. Returns true if this call archived it.
pub fn archive(id: &str) -> Result<bool, String> {
    with_conn(|conn| archive_with_conn(conn, id))
}

pub fn archive_with_conn(conn: &Connection, id: &str) -> Result<bool, String> {
    let now = now_ts();
    let changed = conn
        .execute(
            "UPDATE memories SET archived_at = ?1 WHERE id = ?2 AND archived_at IS NULL",
            params![now, id],
        )
        .map_err(|e| format!("archive: {}", e))?;
    Ok(changed > 0)
}

/// Restore an archived memory back into recall.
pub fn unarchive(id: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "UPDATE memories SET archived_at = NULL WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("unarchive: {}", e))?;
        Ok(())
    })
}

pub fn delete(id: &str) -> Result<(), String> {
    // Flag dependents before deletion (CASCADE will remove edges)
    flag_dependents_for_review(id);

    with_conn(|conn| {
        // Both deletes in one transaction so we never leave a memory deleted
        // with its embedding still present (or vice versa).
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("delete tx: {}", e))?;
        tx.execute("DELETE FROM memories WHERE id = ?1", params![id])
            .map_err(|e| format!("delete: {}", e))?;
        // Keep the vector index in sync. vec_memories is a vec0 virtual table
        // with no FK cascade, so its row must be removed explicitly — otherwise
        // it lingers as an orphan embedding that wastes index space and can
        // occupy slots in KNN search, pushing real hits out of the top-K.
        tx.execute("DELETE FROM vec_memories WHERE memory_id = ?1", params![id])
            .map_err(|e| format!("delete vec: {}", e))?;
        tx.commit().map_err(|e| format!("delete commit: {}", e))?;
        Ok(())
    })
}

pub fn bulk_delete(ids: &[String]) -> Result<usize, String> {
    if ids.is_empty() {
        return Ok(0);
    }
    for id in ids {
        flag_dependents_for_review(id);
    }
    with_conn(|conn| {
        // One transaction for the memories delete plus all vector deletes, so a
        // partial failure can't leave memories gone with their embeddings behind.
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("bulk_delete tx: {}", e))?;
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "DELETE FROM memories WHERE id IN ({})",
            placeholders.join(", ")
        );
        let count = tx
            .execute(&sql, rusqlite::params_from_iter(ids.iter()))
            .map_err(|e| format!("bulk_delete: {}", e))?;
        // Mirror the deletion into the vector index (vec0 has no FK cascade).
        // Point-delete by PK per id — vec0 DELETE is reliable on the primary
        // key but not with an IN (...) list.
        for id in ids {
            tx.execute("DELETE FROM vec_memories WHERE memory_id = ?1", params![id])
                .map_err(|e| format!("bulk_delete vec: {}", e))?;
        }
        tx.commit().map_err(|e| format!("bulk_delete commit: {}", e))?;
        Ok(count)
    })
}

pub fn list_all() -> Result<Vec<Memory>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT * FROM memories ORDER BY updated_at DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], row_to_memory)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

pub fn list_by_topic(topic: &str) -> Result<Vec<Memory>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT * FROM memories WHERE topic = ?1 ORDER BY updated_at DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![topic], row_to_memory)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

pub fn list_untopiced() -> Result<Vec<Memory>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT * FROM memories WHERE topic IS NULL ORDER BY created_at DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], row_to_memory)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

/// Number of memories still awaiting classification. Cheap counterpart to
/// `list_untopiced` for the auto-organize trigger, which only needs the count.
pub fn count_untopiced() -> Result<i64, String> {
    with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM memories WHERE topic IS NULL", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())
    })
}

/// List memories created/updated within a time window (unix timestamps).
pub fn list_since(since_ts: i64, limit: usize) -> Result<Vec<Memory>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT * FROM memories WHERE updated_at >= ?1 AND archived_at IS NULL ORDER BY updated_at DESC LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![since_ts, limit as i64], row_to_memory)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

pub fn list_topics_changed_since(since_ts: i64) -> Result<Vec<String>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT topic FROM memories WHERE topic IS NOT NULL AND (created_at > ?1 OR updated_at > ?1)"
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![since_ts], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

const SETTING_STALE_REVIEW_QUEUE: &str = "stale_review_queue";

/// Flag memories connected via "depends-on" or "supersedes" edges for staleness review.
/// Stores flagged IDs as a JSON array in the settings table.
fn flag_dependents_for_review(memory_id: &str) {
    let connected = match edges::get_neighbors(memory_id) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut flagged_ids: Vec<String> = Vec::new();
    for edge in &connected {
        if edge.edge_type == "depends-on" || edge.edge_type == "supersedes" {
            let other = if edge.source_id == memory_id {
                &edge.target_id
            } else {
                &edge.source_id
            };
            if !flagged_ids.contains(other) {
                flagged_ids.push(other.clone());
            }
        }
    }

    if flagged_ids.is_empty() {
        return;
    }

    // Merge with existing queue
    let existing = settings::get(SETTING_STALE_REVIEW_QUEUE, "[]").unwrap_or_else(|_| "[]".to_string());
    let mut queue: Vec<String> = serde_json::from_str(&existing).unwrap_or_default();
    for id in flagged_ids {
        if !queue.contains(&id) {
            queue.push(id);
        }
    }

    let _ = settings::set(SETTING_STALE_REVIEW_QUEUE, &serde_json::to_string(&queue).unwrap_or_default());
}

/// Fetch multiple memories by ID in a single query.
/// Used by the hook for batch-fetching graph neighbors.
pub fn get_by_ids(ids: &[&str]) -> Result<Vec<Memory>, String> {
    with_conn(|conn| get_by_ids_with_conn(conn, ids))
}

pub fn get_by_ids_with_conn(conn: &Connection, ids: &[&str]) -> Result<Vec<Memory>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{}", i)).collect();
    // Graph-neighbour hydration (the only callers) — archived memories must not
    // resurface through the relationship graph.
    let sql = format!(
        "SELECT * FROM memories WHERE archived_at IS NULL AND id IN ({})",
        placeholders.join(", ")
    );

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare get_by_ids: {}", e))?;

    let rows = stmt
        .query_map(rusqlite::params_from_iter(ids.iter().copied()), row_to_memory)
        .map_err(|e| format!("query get_by_ids: {}", e))?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn count() -> Result<i64, String> {
    with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .map_err(|e| e.to_string())
    })
}

/// Full-text search returning snippets. Used by both the UI and the MCP server.
/// `limit` defaults to 10 if None.
pub fn search(query: &str, limit: Option<u32>) -> Result<Vec<SearchHit>, String> {
    with_conn(|conn| search_with_conn(conn, query, limit))
}

pub fn search_standing_rules(query: &str, limit: Option<u32>) -> Result<Vec<SearchHit>, String> {
    with_conn(|conn| search_standing_rules_with_conn(conn, query, limit))
}

pub fn search_with_conn(
    conn: &Connection,
    query: &str,
    limit: Option<u32>,
) -> Result<Vec<SearchHit>, String> {
    search_filtered(conn, query, limit, false)
}

/// Same search restricted to the types that carry standing instructions.
/// Retrieval runs this as a second lane: a general query is dominated by the
/// narrower, more specific wording of project notes, so a rule that governs
/// the whole question can otherwise miss the candidate pool entirely.
pub fn search_standing_rules_with_conn(
    conn: &Connection,
    query: &str,
    limit: Option<u32>,
) -> Result<Vec<SearchHit>, String> {
    search_filtered(conn, query, limit, true)
}

fn search_filtered(
    conn: &Connection,
    query: &str,
    limit: Option<u32>,
    standing_rules_only: bool,
) -> Result<Vec<SearchHit>, String> {
    let limit = limit.unwrap_or(10).min(50);
    let sanitized = sanitize_fts_query(query);
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }

    let type_clause = if standing_rules_only {
        "AND m.memory_type IN ('user', 'feedback')"
    } else {
        ""
    };

    let mut stmt = conn
        .prepare(&format!(
            r#"SELECT m.id, m.title, m.description, m.topic, m.memory_type, m.project,
                      m.access_count, m.updated_at, m.content,
                      snippet(memories_fts, 2, '[', ']', '...', 32) as snippet,
                      bm25(memories_fts) as score
               FROM memories_fts
               JOIN memories m ON m.rowid = memories_fts.rowid
               WHERE memories_fts MATCH ?1 AND m.archived_at IS NULL {}
               ORDER BY score
               LIMIT ?2"#,
            type_clause
        ))
        .map_err(|e| format!("prepare search: {}", e))?;

    let rows = stmt
        .query_map(params![sanitized, limit as i64], |row| {
            Ok(SearchHit {
                id: row.get("id")?,
                title: row.get("title")?,
                description: row.get("description")?,
                topic: row.get("topic")?,
                memory_type: row.get("memory_type")?,
                project: row.get("project")?,
                snippet: row.get("snippet")?,
                score: row.get("score")?,
                access_count: row.get("access_count")?,
                updated_at: row.get("updated_at")?,
                content: row.get("content")?,
            })
        })
        .map_err(|e| format!("query search: {}", e))?;

    let mut hits = Vec::new();
    for r in rows {
        hits.push(r.map_err(|e| e.to_string())?);
    }

    Ok(hits)
}

/// Increment `access_count` for a batch of memories in a single statement.
/// Call this only for memories that were actually injected or returned to
/// Claude — not the FTS over-fetch pool.
/// Best-effort — errors are swallowed (missing counter updates are not fatal
/// and we don't want to fail retrieval on a write hiccup).
pub fn bump_access(ids: &[&str]) -> Result<(), String> {
    with_conn(|conn| {
        bump_access_counts(conn, ids);
        Ok(())
    })
}

pub(crate) fn bump_access_counts(conn: &Connection, ids: &[&str]) {
    if ids.is_empty() {
        return;
    }
    let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{}", i)).collect();
    let sql = format!(
        "UPDATE memories SET access_count = access_count + 1 WHERE id IN ({})",
        placeholders.join(", ")
    );
    let _ = conn.execute(&sql, rusqlite::params_from_iter(ids.iter().copied()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_fts_query() {
        assert_eq!(sanitize_fts_query(""), "");
        assert_eq!(sanitize_fts_query("docker"), "\"docker\"*");
        assert_eq!(
            sanitize_fts_query("docker deploy"),
            "\"deploy\"* OR \"docker\"*"
        );
        assert_eq!(
            sanitize_fts_query("docker; DROP TABLE"),
            "\"docker\"* OR \"table\"* OR \"drop\"*"
        );
        assert_eq!(sanitize_fts_query("a docker"), "\"docker\"*");
        // Natural-language stopwords are dropped so BM25 is not run over the
        // whole corpus. Legacy kept every token (see proof test below).
        assert_eq!(
            sanitize_fts_query("what do you use for postgres"),
            "\"postgres\"*"
        );
        // Hyphens become spaces — FTS5 would otherwise treat "red-roof" as a
        // column filter (`no such column: roof`).
        let hyphen = sanitize_fts_query("red-roof tiles");
        assert!(
            !hyphen.contains('-'),
            "hyphen must not survive as an operator: {hyphen}"
        );
        assert!(hyphen.contains("red") || hyphen.contains("roof"));
        // Short technical tokens stay exact (no prefix wildcard).
        assert_eq!(sanitize_fts_query("wal e2e"), "\"e2e\" OR \"wal\"");
    }

    #[test]
    fn proof_new_sanitizer_drops_stopwords_legacy_does_not() {
        let prompt = "what do you use for postgres";
        let legacy = sanitize_fts_query_legacy(prompt);
        let next = sanitize_fts_query(prompt);
        assert!(
            legacy.contains("what*") && legacy.contains("you*") && legacy.contains("for*"),
            "legacy baseline drifted: {legacy}"
        );
        assert_eq!(next, "\"postgres\"*");
        assert!(
            next.split(" OR ").count() < legacy.split(" OR ").count(),
            "new query must be stricter than legacy"
        );
    }

    #[test]
    fn proof_hyphenated_query_is_valid_fts_legacy_is_not() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE memories (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                content TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                memory_type TEXT,
                topic TEXT,
                source TEXT,
                project TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                access_count INTEGER NOT NULL DEFAULT 0,
                archived_at INTEGER
            );
            CREATE VIRTUAL TABLE memories_fts USING fts5(
                title, description, content,
                content='memories', content_rowid='rowid',
                tokenize='porter unicode61'
            );
            CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, title, description, content)
                VALUES (new.rowid, new.title, new.description, new.content);
            END;
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memories (id, title, description, content, content_hash, created_at, updated_at)
             VALUES ('1', 'Red roof tiles', '', 'The cottage uses red-roof clay tiles', 'h', 0, 0)",
            [],
        )
        .unwrap();

        let legacy_err = conn
            .prepare("SELECT m.id FROM memories_fts JOIN memories m ON m.rowid = memories_fts.rowid WHERE memories_fts MATCH ?1")
            .and_then(|mut stmt| {
                stmt.query_row([sanitize_fts_query_legacy("red-roof")], |r| {
                    r.get::<_, String>(0)
                })
            })
            .expect_err("legacy hyphen query must fail FTS parse");
        assert!(
            legacy_err.to_string().contains("no such column")
                || legacy_err.to_string().to_lowercase().contains("syntax"),
            "unexpected legacy error: {legacy_err}"
        );

        let mut stmt = conn
            .prepare("SELECT m.id FROM memories_fts JOIN memories m ON m.rowid = memories_fts.rowid WHERE memories_fts MATCH ?1")
            .unwrap();
        let id: String = stmt
            .query_row([sanitize_fts_query("red-roof")], |r| r.get(0))
            .expect("new hyphen query must be valid FTS");
        assert_eq!(id, "1");
    }

    #[test]
    fn proof_stopword_or_matches_almost_everything_new_does_not() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE memories (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                content TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                memory_type TEXT,
                topic TEXT,
                source TEXT,
                project TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                access_count INTEGER NOT NULL DEFAULT 0,
                archived_at INTEGER
            );
            CREATE VIRTUAL TABLE memories_fts USING fts5(
                title, description, content,
                content='memories', content_rowid='rowid',
                tokenize='porter unicode61'
            );
            CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, title, description, content)
                VALUES (new.rowid, new.title, new.description, new.content);
            END;
            "#,
        )
        .unwrap();

        let rows = [
            ("pg", "Postgres port", "Production postgres listens on 5432"),
            ("dk", "Docker notes", "We use docker compose for staging"),
            ("ui", "Button copy", "You can change this later if you want"),
            ("gh", "Git habit", "What we do for PRs is squash"),
            ("xx", "Unrelated", "The cat sat on the mat and you can see it"),
        ];
        for (id, title, content) in rows {
            conn.execute(
                "INSERT INTO memories (id, title, description, content, content_hash, created_at, updated_at)
                 VALUES (?1, ?2, '', ?3, ?1, 0, 0)",
                rusqlite::params![id, title, content],
            )
            .unwrap();
        }

        let prompt = "what do you use for postgres";
        let count = |q: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM memories_fts JOIN memories m ON m.rowid = memories_fts.rowid WHERE memories_fts MATCH ?1",
                [q],
                |r| r.get(0),
            )
            .unwrap()
        };

        let legacy_hits = count(&sanitize_fts_query_legacy(prompt));
        let new_hits = count(&sanitize_fts_query(prompt));
        assert!(
            legacy_hits >= 4,
            "legacy OR-of-stopwords should flood the corpus, got {legacy_hits}"
        );
        assert_eq!(
            new_hits, 1,
            "new sanitizer should keep only the postgres row, got {new_hits}"
        );
    }

    #[test]
    fn test_content_hash_deterministic() {
        assert_eq!(content_hash("hello"), content_hash("hello"));
        assert_ne!(content_hash("hello"), content_hash("world"));
    }
}

/// Words that match almost every English memory. OR-ing them with a prefix
/// wildcard makes BM25 rank the whole corpus. Kept small and ASCII-only —
/// technical tokens (`wal`, `e2e`, `k8s`) are never on this list.
const FTS_STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "can", "do", "for",
    "from", "had", "has", "have", "how", "i", "if", "in", "into", "is", "it",
    "its", "me", "my", "no", "not", "of", "on", "or", "our", "so", "than",
    "that", "the", "then", "there", "this", "to", "too", "up", "us", "use",
    "used", "using", "was", "we", "were", "what", "when", "where", "which",
    "who", "why", "will", "with", "would", "you", "your",
];

const FTS_MAX_TERMS: usize = 12;
const FTS_PREFIX_MIN_LEN: usize = 4;

/// Escape/sanitize a user query for FTS5.
///
/// - Hyphens become spaces so `red-roof` cannot be parsed as a column filter
/// - Stopwords and 1-char tokens are dropped
/// - At most `FTS_MAX_TERMS` tokens, preferring longer / rarer-looking words
/// - Prefix `*` only on tokens ≥ 4 chars; short tokens stay exact
/// - Each token is quoted so leftover punctuation cannot become an operator
/// - Joins with `OR` (FTS5 defaults to AND — we want forgiving recall)
pub(crate) fn sanitize_fts_query(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() || c == '_' {
                c
            } else {
                // Including '-': FTS5 treats `foo-bar` as a column filter.
                ' '
            }
        })
        .collect();

    let mut seen = std::collections::HashSet::new();
    let mut words: Vec<String> = Vec::new();
    for raw in cleaned.split_whitespace() {
        let w = raw.to_ascii_lowercase();
        if w.len() <= 1 || FTS_STOPWORDS.contains(&w.as_str()) {
            continue;
        }
        if seen.insert(w.clone()) {
            words.push(w);
        }
    }
    if words.is_empty() {
        return String::new();
    }

    words.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    words.truncate(FTS_MAX_TERMS);

    let terms: Vec<String> = words
        .into_iter()
        .map(|w| {
            if w.len() >= FTS_PREFIX_MIN_LEN {
                format_fts_term(&format!("{w}*"))
            } else {
                format_fts_term(&w)
            }
        })
        .collect();

    terms.join(" OR ")
}

fn format_fts_term(term: &str) -> String {
    // Quote every token. FTS5 quoted-prefix (`"postgres"*`) is valid and
    // prevents leftover punctuation from becoming an operator.
    if let Some(stripped) = term.strip_suffix('*') {
        format!("\"{}\"*", stripped.replace('"', ""))
    } else {
        format!("\"{}\"", term.replace('"', ""))
    }
}

/// Pre-2026-08 sanitizer, kept only so proof tests can lock the regression
/// the new function is supposed to fix. Do not call from production paths.
#[cfg(test)]
pub(crate) fn sanitize_fts_query_legacy(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() || c == '_' || c == '-' {
                c
            } else {
                ' '
            }
        })
        .collect();

    let words: Vec<&str> = cleaned.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }

    let terms: Vec<String> = words
        .iter()
        .filter(|w| w.len() > 1)
        .map(|w| format!("{}*", w))
        .collect();

    if terms.is_empty() {
        return String::new();
    }

    terms.join(" OR ")
}
