pub mod edges;
pub mod history;
pub mod memories;
pub mod settings;
pub mod topics;

#[cfg(test)]
mod smoke_test;

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Path to the SQLite database file.
///
/// Lookup order:
/// 1. `CLAUDE_MEMORY_DB_DIR` environment variable (escape hatch for users whose
///    endpoint protection tools monitor the default hidden home directory)
/// 2. `~/.claude-memory-manager/` (default)
///
/// The DB will be migrated from older locations if found.
pub fn db_path() -> PathBuf {
    let data_dir = resolve_data_dir();
    std::fs::create_dir_all(&data_dir).ok();

    let new_db = data_dir.join("memories.db");
    if !new_db.exists() {
        migrate_from_old_locations(&data_dir);
    }

    new_db
}

fn resolve_data_dir() -> PathBuf {
    // Escape hatch for endpoint-protection tools like WithSecure XFENCE
    if let Ok(custom) = std::env::var("CLAUDE_MEMORY_DB_DIR") {
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }

    dirs::home_dir()
        .expect("no home dir available")
        .join(".claude-memory-manager")
}

fn migrate_from_old_locations(new_dir: &std::path::Path) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return,
    };

    let candidates = [
        // Previous default: ~/.claude-memory-manager (same as current, no-op)
        // ~/Library/Application Support/claude-memory-manager (macOS original)
        dirs::data_local_dir().map(|d| d.join("claude-memory-manager")),
        // ~/.claude-memory-manager fallback
        Some(home.join(".claude-memory-manager")),
    ];

    for src_dir in candidates.iter().flatten() {
        if src_dir == new_dir {
            continue;
        }
        let src_db = src_dir.join("memories.db");
        if src_db.exists() {
            for name in ["memories.db", "memories.db-wal", "memories.db-shm"] {
                let src = src_dir.join(name);
                if src.exists() {
                    let _ = std::fs::rename(&src, new_dir.join(name));
                }
            }
            return;
        }
    }
}

/// Global connection, protected by Mutex for thread safety.
/// SQLite in WAL mode allows concurrent readers; we serialize writes.
static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub fn init() -> Result<(), String> {
    let conn = open_connection()?;
    DB.set(Mutex::new(conn))
        .map_err(|_| "DB already initialized".to_string())?;
    Ok(())
}

pub fn with_conn<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce(&Connection) -> Result<R, String>,
{
    let db = DB.get().ok_or_else(|| "DB not initialized".to_string())?;
    let conn = db.lock().map_err(|e| format!("DB lock poisoned: {}", e))?;
    f(&conn)
}

/// Flush WAL to disk before the process exits.
pub fn shutdown() {
    if let Some(db) = DB.get() {
        if let Ok(conn) = db.lock() {
            let _ = conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
        }
    }
}

fn open_connection() -> Result<Connection, String> {
    let path = db_path();
    let conn = Connection::open(&path)
        .map_err(|e| format!("Failed to open DB at {}: {}", path.display(), e))?;

    // Busy timeout so queries wait instead of failing immediately
    conn.pragma_update(None, "busy_timeout", "5000")
        .map_err(|e| format!("busy_timeout: {}", e))?;

    // WAL mode for concurrent reads with the MCP server
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| format!("WAL mode: {}", e))?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| format!("synchronous: {}", e))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| format!("foreign_keys: {}", e))?;

    run_migrations(&conn)?;

    // Checkpoint WAL to ensure data from previous sessions is flushed to the
    // main database file. This prevents stale WAL state after a crash or
    // force-quit from blocking reads on startup.
    let _ = conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");

    Ok(conn)
}

fn run_migrations(conn: &Connection) -> Result<(), String> {
    // Version tracking
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);",
    )
    .map_err(|e| format!("schema_version: {}", e))?;

    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if version < 1 {
        apply_migration_v1(conn)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])
            .map_err(|e| format!("bump v1: {}", e))?;
    }

    if version < 2 {
        apply_migration_v2(conn)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (2)", [])
            .map_err(|e| format!("bump v2: {}", e))?;
    }

    Ok(())
}

fn apply_migration_v2(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
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

        CREATE INDEX idx_edges_source ON memory_edges(source_id);
        CREATE INDEX idx_edges_target ON memory_edges(target_id);
        CREATE INDEX idx_edges_type ON memory_edges(edge_type);
        "#,
    )
    .map_err(|e| format!("migration v2: {}", e))
}

fn apply_migration_v1(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
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
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            access_count INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY(topic) REFERENCES topics(name) ON DELETE SET NULL
        );

        CREATE INDEX idx_memories_topic ON memories(topic);
        CREATE INDEX idx_memories_updated ON memories(updated_at);
        CREATE INDEX idx_memories_hash ON memories(content_hash);

        -- FTS5 virtual table with porter stemming
        CREATE VIRTUAL TABLE memories_fts USING fts5(
            title,
            description,
            content,
            content='memories',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );

        -- Sync triggers
        CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, title, description, content)
            VALUES (new.rowid, new.title, new.description, new.content);
        END;

        CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, title, description, content)
            VALUES ('delete', old.rowid, old.title, old.description, old.content);
        END;

        CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, title, description, content)
            VALUES ('delete', old.rowid, old.title, old.description, old.content);
            INSERT INTO memories_fts(rowid, title, description, content)
            VALUES (new.rowid, new.title, new.description, new.content);
        END;
        "#,
    )
    .map_err(|e| format!("migration v1: {}", e))
}
