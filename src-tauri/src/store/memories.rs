use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::with_conn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub title: String,
    pub description: String,
    pub content: String,
    pub memory_type: Option<String>,
    pub topic: Option<String>,
    pub source: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub access_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub description: String,
    pub snippet: String,
    pub topic: Option<String>,
    pub memory_type: Option<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMemory {
    pub title: String,
    pub description: String,
    pub content: String,
    pub memory_type: Option<String>,
    pub topic: Option<String>,
    pub source: Option<String>,
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
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        access_count: row.get("access_count")?,
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
    with_conn(|conn| {
        let hash = content_hash(&new.content);

        // Check for existing identical content
        if let Some(existing) = find_by_hash(conn, &hash)? {
            return Ok(existing);
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = now_ts();

        conn.execute(
            r#"INSERT INTO memories
               (id, title, description, content, content_hash, memory_type, topic, source, created_at, updated_at, access_count)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)"#,
            params![
                id,
                new.title,
                new.description,
                new.content,
                hash,
                new.memory_type,
                new.topic,
                new.source,
                now,
                now,
            ],
        )
        .map_err(|e| format!("insert: {}", e))?;

        get_by_id(conn, &id)?.ok_or_else(|| "insert succeeded but row missing".to_string())
    })
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

        get_by_id(conn, id)?.ok_or_else(|| format!("memory {} not found after update", id))
    })
}

pub fn delete(id: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute("DELETE FROM memories WHERE id = ?1", params![id])
            .map_err(|e| format!("delete: {}", e))?;
        Ok(())
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

pub fn count() -> Result<i64, String> {
    with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .map_err(|e| e.to_string())
    })
}

/// Full-text search returning snippets. Used by both the UI and the MCP server.
/// `limit` defaults to 10 if None.
pub fn search(query: &str, limit: Option<u32>) -> Result<Vec<SearchHit>, String> {
    with_conn(|conn| {
        let limit = limit.unwrap_or(10).min(50);
        let sanitized = sanitize_fts_query(query);
        if sanitized.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = conn
            .prepare(
                r#"SELECT m.id, m.title, m.description, m.topic, m.memory_type,
                          snippet(memories_fts, 2, '[', ']', '...', 32) as snippet,
                          bm25(memories_fts) as score
                   FROM memories_fts
                   JOIN memories m ON m.rowid = memories_fts.rowid
                   WHERE memories_fts MATCH ?1
                   ORDER BY score
                   LIMIT ?2"#,
            )
            .map_err(|e| format!("prepare search: {}", e))?;

        let rows = stmt
            .query_map(params![sanitized, limit as i64], |row| {
                Ok(SearchHit {
                    id: row.get("id")?,
                    title: row.get("title")?,
                    description: row.get("description")?,
                    topic: row.get("topic")?,
                    memory_type: row.get("memory_type")?,
                    snippet: row.get("snippet")?,
                    score: row.get("score")?,
                })
            })
            .map_err(|e| format!("query search: {}", e))?;

        let mut hits = Vec::new();
        for r in rows {
            hits.push(r.map_err(|e| e.to_string())?);
        }

        // Bump access count for returned hits
        for hit in &hits {
            let _ = conn.execute(
                "UPDATE memories SET access_count = access_count + 1 WHERE id = ?1",
                params![hit.id],
            );
        }

        Ok(hits)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_fts_query() {
        assert_eq!(sanitize_fts_query(""), "");
        assert_eq!(sanitize_fts_query("docker"), "docker*");
        // Multi-word queries use OR so partial matches work (FTS5 defaults to AND)
        assert_eq!(sanitize_fts_query("docker deploy"), "docker* OR deploy*");
        assert_eq!(
            sanitize_fts_query("docker; DROP TABLE"),
            "docker* OR DROP* OR TABLE*"
        );
        // Single-char words filtered
        assert_eq!(sanitize_fts_query("a docker"), "docker*");
        // Multi-word natural language uses OR across all terms > 1 char
        assert_eq!(
            sanitize_fts_query("what do you use for postgres"),
            "what* OR do* OR you* OR use* OR for* OR postgres*"
        );
    }

    #[test]
    fn test_content_hash_deterministic() {
        assert_eq!(content_hash("hello"), content_hash("hello"));
        assert_ne!(content_hash("hello"), content_hash("world"));
    }
}

/// Escape/sanitize a user query for FTS5.
///
/// - Strips special chars (SQL injection / FTS operator safety)
/// - Filters single-char noise
/// - Wraps each word with a prefix-match wildcard
/// - Joins with `OR` (FTS5 defaults to AND — we want forgiving recall, let
///   bm25 rank by match count/quality)
fn sanitize_fts_query(input: &str) -> String {
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
