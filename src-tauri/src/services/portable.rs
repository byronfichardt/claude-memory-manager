//! Export / import the memory store as a portable JSON bundle.
//!
//! The bundle is versioned (`version = 1`). Importers must tolerate future
//! version bumps by rejecting with a clear error rather than silently
//! corrupting data.
//!
//! Memory deduplication on import uses `content_hash` — identical content
//! across exports is merged onto the existing row, not duplicated. Edge
//! endpoints are remapped through the exported→final id mapping so edges
//! survive dedup.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::store::{edges::MemoryEdge, memories::Memory, topics::Topic, with_conn};

pub const BUNDLE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportBundle {
    pub version: u32,
    pub exported_at: i64,
    pub memory_count: usize,
    pub topics: Vec<Topic>,
    pub memories: Vec<ExportedMemory>,
    pub edges: Vec<MemoryEdge>,
}

/// Like `Memory`, but carries `content_hash` so importers can dedup
/// deterministically without re-hashing (also lets users edit exports
/// by hand without breaking dedup — they can delete the hash field and
/// the importer will recompute it).
#[derive(Debug, Serialize, Deserialize)]
pub struct ExportedMemory {
    pub id: String,
    pub title: String,
    pub description: String,
    pub content: String,
    pub content_hash: Option<String>,
    pub memory_type: Option<String>,
    pub topic: Option<String>,
    pub source: Option<String>,
    pub project: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub access_count: i64,
}

impl From<Memory> for ExportedMemory {
    fn from(m: Memory) -> Self {
        ExportedMemory {
            id: m.id,
            title: m.title,
            description: m.description,
            content: m.content,
            content_hash: None, // Filled in below so we don't fight the From trait
            memory_type: m.memory_type,
            topic: m.topic,
            source: m.source,
            project: m.project,
            created_at: m.created_at,
            updated_at: m.updated_at,
            access_count: m.access_count,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Insert missing topics/memories/edges; existing rows untouched.
    /// Safe, idempotent — re-importing the same bundle is a no-op.
    Merge,
    /// Delete all current data first, then import.
    Replace,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ImportReport {
    pub memories_added: usize,
    pub memories_skipped: usize,
    pub topics_added: usize,
    pub edges_added: usize,
    pub edges_skipped: usize,
    pub errors: Vec<String>,
}

/// Serialize the entire memory store as a JSON bundle.
pub fn build_export() -> Result<String, String> {
    with_conn(|conn| build_export_in(conn))
}

/// Implementation of `build_export` against an arbitrary connection. Useful
/// for unit tests that want to run against an in-memory DB.
pub fn build_export_in(conn: &Connection) -> Result<String, String> {
    let topics = load_topics(conn)?;
    let memories = load_memories_with_hash(conn)?;
    let edges = load_edges(conn)?;
    let memory_count = memories.len();

    let bundle = ExportBundle {
        version: BUNDLE_VERSION,
        exported_at: chrono::Utc::now().timestamp(),
        memory_count,
        topics,
        memories,
        edges,
    };

    serde_json::to_string_pretty(&bundle).map_err(|e| format!("serialize: {}", e))
}

/// Parse a JSON bundle and merge it into the current store.
pub fn import_bundle(json: &str, mode: ImportMode) -> Result<ImportReport, String> {
    with_conn(|conn| import_bundle_in(conn, json, mode))
}

/// Implementation of `import_bundle` against an arbitrary connection.
pub fn import_bundle_in(
    conn: &Connection,
    json: &str,
    mode: ImportMode,
) -> Result<ImportReport, String> {
    let bundle: ExportBundle =
        serde_json::from_str(json).map_err(|e| format!("parse bundle: {}", e))?;

    if bundle.version != BUNDLE_VERSION {
        return Err(format!(
            "unsupported bundle version {} (this app supports version {})",
            bundle.version, BUNDLE_VERSION
        ));
    }

    {
        let mut report = ImportReport::default();

        // Enforce transactional consistency for the whole import.
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| format!("begin tx: {}", e))?;

        let result = (|| -> Result<(), String> {
            if mode == ImportMode::Replace {
                // Order matters: edges first (FKs cascade on delete but being
                // explicit avoids surprises), then memories, then topics.
                conn.execute("DELETE FROM memory_edges", [])
                    .map_err(|e| format!("wipe edges: {}", e))?;
                conn.execute("DELETE FROM memories", [])
                    .map_err(|e| format!("wipe memories: {}", e))?;
                conn.execute("DELETE FROM topics", [])
                    .map_err(|e| format!("wipe topics: {}", e))?;
            }

            // 1. Upsert topics so the memories FK is satisfied.
            for topic in &bundle.topics {
                let before: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM topics WHERE name = ?1",
                        params![topic.name],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                conn.execute(
                    r#"INSERT INTO topics (name, description, color, created_at)
                       VALUES (?1, ?2, ?3, ?4)
                       ON CONFLICT(name) DO UPDATE SET
                           description = COALESCE(excluded.description, topics.description),
                           color       = COALESCE(excluded.color,       topics.color)"#,
                    params![topic.name, topic.description, topic.color, topic.created_at],
                )
                .map_err(|e| format!("upsert topic {}: {}", topic.name, e))?;
                if before == 0 {
                    report.topics_added += 1;
                }
            }

            // 2. Insert memories, mapping exported-id -> final-id so edges
            //    still resolve after dedup. Dedup by content_hash.
            let mut id_map: HashMap<String, String> = HashMap::new();

            for mem in &bundle.memories {
                let hash = mem
                    .content_hash
                    .clone()
                    .unwrap_or_else(|| content_hash(&mem.content));

                // Already present (by hash)? Record mapping, skip insert.
                if let Some(existing_id) = find_id_by_hash(conn, &hash)? {
                    id_map.insert(mem.id.clone(), existing_id);
                    report.memories_skipped += 1;
                    continue;
                }

                // Does the exported ID collide with an existing different row?
                // If so, generate a fresh UUID; otherwise preserve the original
                // id so cross-install references stay stable.
                let final_id = if id_exists(conn, &mem.id)? {
                    uuid::Uuid::new_v4().to_string()
                } else {
                    mem.id.clone()
                };

                conn.execute(
                    r#"INSERT INTO memories
                       (id, title, description, content, content_hash, memory_type,
                        topic, source, project, created_at, updated_at, access_count)
                       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"#,
                    params![
                        final_id,
                        mem.title,
                        mem.description,
                        mem.content,
                        hash,
                        mem.memory_type,
                        mem.topic,
                        mem.source,
                        mem.project,
                        mem.created_at,
                        mem.updated_at,
                        mem.access_count,
                    ],
                )
                .map_err(|e| format!("insert memory {}: {}", mem.id, e))?;

                id_map.insert(mem.id.clone(), final_id);
                report.memories_added += 1;
            }

            // 3. Recreate edges with remapped endpoints.
            for edge in &bundle.edges {
                let src = match id_map.get(&edge.source_id) {
                    Some(s) => s.clone(),
                    None if id_exists(conn, &edge.source_id)? => edge.source_id.clone(),
                    _ => {
                        report.edges_skipped += 1;
                        continue;
                    }
                };
                let tgt = match id_map.get(&edge.target_id) {
                    Some(t) => t.clone(),
                    None if id_exists(conn, &edge.target_id)? => edge.target_id.clone(),
                    _ => {
                        report.edges_skipped += 1;
                        continue;
                    }
                };

                let changed = conn
                    .execute(
                        r#"INSERT INTO memory_edges
                           (source_id, target_id, edge_type, weight, source_origin, created_at, updated_at)
                           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                           ON CONFLICT(source_id, target_id, edge_type) DO UPDATE SET
                               weight = MAX(memory_edges.weight, excluded.weight),
                               updated_at = excluded.updated_at"#,
                        params![
                            src,
                            tgt,
                            edge.edge_type,
                            edge.weight,
                            edge.source_origin,
                            edge.created_at,
                            edge.updated_at,
                        ],
                    )
                    .map_err(|e| format!("insert edge: {}", e))?;
                if changed > 0 {
                    report.edges_added += 1;
                }
            }

            Ok(())
        })();

        match result {
            Ok(_) => {
                conn.execute_batch("COMMIT")
                    .map_err(|e| format!("commit: {}", e))?;
                Ok(report)
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

fn content_hash(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn find_id_by_hash(conn: &Connection, hash: &str) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare("SELECT id FROM memories WHERE content_hash = ?1 LIMIT 1")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query(params![hash]).map_err(|e| e.to_string())?;
    match rows.next().map_err(|e| e.to_string())? {
        Some(row) => Ok(Some(row.get(0).map_err(|e| e.to_string())?)),
        None => Ok(None),
    }
}

fn id_exists(conn: &Connection, id: &str) -> Result<bool, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM memories WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(count > 0)
}

fn load_topics(conn: &Connection) -> Result<Vec<Topic>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT name, description, color, created_at FROM topics ORDER BY name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Topic {
                name: row.get("name")?,
                description: row.get("description")?,
                color: row.get("color")?,
                created_at: row.get("created_at")?,
                // memory_count is a computed column used by the UI; set 0 on
                // export since the import path ignores it anyway.
                memory_count: 0,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn load_memories_with_hash(conn: &Connection) -> Result<Vec<ExportedMemory>, String> {
    let mut stmt = conn
        .prepare(
            r#"SELECT id, title, description, content, content_hash, memory_type,
                      topic, source, project, created_at, updated_at, access_count
               FROM memories ORDER BY created_at"#,
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ExportedMemory {
                id: row.get("id")?,
                title: row.get("title")?,
                description: row.get("description")?,
                content: row.get("content")?,
                content_hash: row.get("content_hash")?,
                memory_type: row.get("memory_type")?,
                topic: row.get("topic")?,
                source: row.get("source")?,
                project: row.get("project")?,
                created_at: row.get("created_at")?,
                updated_at: row.get("updated_at")?,
                access_count: row.get("access_count")?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn load_edges(conn: &Connection) -> Result<Vec<MemoryEdge>, String> {
    let mut stmt = conn
        .prepare(
            r#"SELECT id, source_id, target_id, edge_type, weight, source_origin,
                      created_at, updated_at
               FROM memory_edges ORDER BY id"#,
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
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
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_schema(conn: &Connection) {
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;

            CREATE TABLE topics (
                name TEXT PRIMARY KEY,
                description TEXT,
                color TEXT,
                created_at INTEGER NOT NULL
            );

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
                FOREIGN KEY(topic) REFERENCES topics(name) ON DELETE SET NULL
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

            CREATE TABLE memory_edges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                source_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                edge_type TEXT NOT NULL,
                weight REAL NOT NULL DEFAULT 0.5,
                source_origin TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY(source_id) REFERENCES memories(id) ON DELETE CASCADE,
                FOREIGN KEY(target_id) REFERENCES memories(id) ON DELETE CASCADE,
                UNIQUE(source_id, target_id, edge_type)
            );
            "#,
        )
        .unwrap();
    }

    fn seed(conn: &Connection) {
        conn.execute(
            "INSERT INTO topics (name, description, color, created_at) VALUES ('deploy', 'deployment stuff', '#abc', 100)",
            [],
        )
        .unwrap();

        for (id, title, content, topic) in [
            ("m1", "Docker deploy", "we use docker compose for staging", Some("deploy")),
            ("m2", "Kamal prod", "kamal for production deploys", Some("deploy")),
            ("m3", "Terse output", "byron prefers short answers", None),
        ] {
            conn.execute(
                r#"INSERT INTO memories (id, title, description, content, content_hash, topic, created_at, updated_at)
                   VALUES (?1, ?2, '', ?3, ?4, ?5, 100, 100)"#,
                params![id, title, content, content_hash(content), topic],
            )
            .unwrap();
        }

        conn.execute(
            r#"INSERT INTO memory_edges (source_id, target_id, edge_type, weight, source_origin, created_at, updated_at)
               VALUES ('m1', 'm2', 'relates-to', 0.75, 'ai_discovered', 100, 100)"#,
            [],
        )
        .unwrap();
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {}", table), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn export_import_roundtrip_preserves_counts() {
        // Source DB: seed and export
        let source = Connection::open_in_memory().unwrap();
        setup_schema(&source);
        seed(&source);

        let json = build_export_in(&source).unwrap();
        assert!(json.contains("\"version\""));
        assert!(json.contains("\"m1\""));

        // Target DB: import the bundle into a fresh schema
        let target = Connection::open_in_memory().unwrap();
        setup_schema(&target);

        let report = import_bundle_in(&target, &json, ImportMode::Merge).unwrap();
        assert_eq!(report.memories_added, 3);
        assert_eq!(report.topics_added, 1);
        assert_eq!(report.edges_added, 1);
        assert_eq!(count(&target, "memories"), 3);
        assert_eq!(count(&target, "topics"), 1);
        assert_eq!(count(&target, "memory_edges"), 1);
    }

    #[test]
    fn import_is_idempotent_on_reimport() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        seed(&conn);

        let json = build_export_in(&conn).unwrap();

        // Re-importing against the same DB must not duplicate anything.
        let report = import_bundle_in(&conn, &json, ImportMode::Merge).unwrap();
        assert_eq!(report.memories_added, 0);
        assert_eq!(report.memories_skipped, 3);
        assert_eq!(count(&conn, "memories"), 3);
        assert_eq!(count(&conn, "memory_edges"), 1);
    }

    #[test]
    fn replace_mode_wipes_before_import() {
        let target = Connection::open_in_memory().unwrap();
        setup_schema(&target);
        // Pre-existing data that should disappear
        target
            .execute(
                r#"INSERT INTO memories (id, title, description, content, content_hash, created_at, updated_at)
                   VALUES ('old', 'old', '', 'old content', 'h_old', 0, 0)"#,
                [],
            )
            .unwrap();
        assert_eq!(count(&target, "memories"), 1);

        // Build a bundle from a different source
        let source = Connection::open_in_memory().unwrap();
        setup_schema(&source);
        seed(&source);
        let json = build_export_in(&source).unwrap();

        import_bundle_in(&target, &json, ImportMode::Replace).unwrap();
        assert_eq!(count(&target, "memories"), 3);
        // Original "old" row must be gone
        let still_there: i64 = target
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE id = 'old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still_there, 0);
    }

    #[test]
    fn exported_memory_from_memory_drops_hash() {
        let mem = Memory {
            id: "id1".to_string(),
            title: "t".to_string(),
            description: "d".to_string(),
            content: "c".to_string(),
            memory_type: None,
            topic: None,
            source: None,
            project: None,
            created_at: 1,
            updated_at: 2,
            access_count: 3,
        };
        let exp: ExportedMemory = mem.into();
        assert_eq!(exp.id, "id1");
        assert!(exp.content_hash.is_none());
    }

    #[test]
    fn import_rejects_future_version() {
        let json = serde_json::json!({
            "version": 999,
            "exported_at": 0,
            "memory_count": 0,
            "topics": [],
            "memories": [],
            "edges": []
        })
        .to_string();
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);
        let err = import_bundle_in(&conn, &json, ImportMode::Merge).unwrap_err();
        assert!(err.contains("unsupported bundle version"));
    }

    #[test]
    fn content_hash_matches_memories_module() {
        // Same algorithm as store::memories::content_hash must produce the
        // same output, or dedup on import won't match rows written by the
        // normal insert path.
        let h1 = content_hash("hello world");
        // Reference value: sha256("hello world")
        assert_eq!(
            h1,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
