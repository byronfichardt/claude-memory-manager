pub mod edges;
pub mod history;
pub mod memories;
pub mod settings;
pub mod topics;

#[cfg(test)]
mod smoke_test;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

/// Captured startup errors (directory creation, DB init) that the UI can
/// surface. Populated by `db_path` / `data_dir` / `init`.
static STARTUP_ERRORS: OnceLock<RwLock<Vec<String>>> = OnceLock::new();

fn startup_errors() -> &'static RwLock<Vec<String>> {
    STARTUP_ERRORS.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn record_startup_error(msg: impl Into<String>) {
    let msg = msg.into();
    eprintln!("[claude-memory-manager startup] {}", msg);
    if let Ok(mut errs) = startup_errors().write() {
        errs.push(msg.clone());
    }
    append_startup_log(&msg);
}

pub fn get_startup_errors() -> Vec<String> {
    startup_errors()
        .read()
        .map(|errs| errs.clone())
        .unwrap_or_default()
}

/// Append a line to `<data_dir>/startup.log`. Best-effort: if we can't reach
/// the data dir (the very thing that may have failed), fall back to the home
/// directory, then silently give up. Never panics.
fn append_startup_log(msg: &str) {
    use std::io::Write;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{} {}\n", ts, msg);

    // Try the resolved data dir first (without triggering recursive error recording)
    let data_dir_candidate = resolve_data_dir_raw();
    let candidates: [Option<PathBuf>; 2] = [
        data_dir_candidate.map(|d| d.join("startup.log")),
        dirs::home_dir().map(|h| h.join(".claude-memory-manager-startup.log")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if let Some(parent) = candidate.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&candidate)
        {
            if f.write_all(line.as_bytes()).is_ok() {
                return;
            }
        }
    }
}

/// Path to the SQLite database file.
///
/// Lookup order:
/// 1. `CLAUDE_MEMORY_DB_DIR` environment variable (escape hatch for users whose
///    endpoint protection tools monitor the default hidden home directory)
/// 2. `~/.claude-memory-manager/` (default)
///
/// The DB will be migrated from older locations if found.
pub fn db_path() -> PathBuf {
    let data_dir = ensure_data_dir();

    let new_db = data_dir.join("memories.db");
    if !new_db.exists() {
        migrate_from_old_locations(&data_dir);
    }

    new_db
}

/// Data directory holding `memories.db` and sidecar state files
/// (e.g. `active-project.json`). Created if missing.
pub fn data_dir() -> PathBuf {
    ensure_data_dir()
}

fn ensure_data_dir() -> PathBuf {
    let dir = resolve_data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        record_startup_error(format!("Failed to create data dir {}: {}", dir.display(), e));
    }
    dir
}

fn resolve_data_dir() -> PathBuf {
    resolve_data_dir_raw().unwrap_or_else(|| {
        record_startup_error(
            "No home directory available; falling back to current working directory",
        );
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".claude-memory-manager")
    })
}

/// Same as resolve_data_dir, but returns None instead of recording an error.
/// Used internally by the error-recording machinery to avoid recursion.
fn resolve_data_dir_raw() -> Option<PathBuf> {
    // Escape hatch for endpoint-protection tools like WithSecure XFENCE
    if let Ok(custom) = std::env::var("CLAUDE_MEMORY_DB_DIR") {
        if !custom.is_empty() {
            return Some(PathBuf::from(custom));
        }
    }

    dirs::home_dir().map(|h| h.join(".claude-memory-manager"))
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

/// Connection pool. WAL mode allows concurrent readers when each caller
/// holds its own connection — a single Mutex<Connection> serializes
/// everything (including reads), which stalls the UI when the organizer
/// is writing.
static POOL: OnceLock<Pool<SqliteConnectionManager>> = OnceLock::new();

pub fn init() -> Result<(), String> {
    let pool = build_pool().map_err(|e| {
        record_startup_error(format!("DB init failed: {}", e));
        e
    })?;

    // Run migrations and an initial WAL checkpoint on a fresh connection.
    let conn = pool.get().map_err(|e| format!("DB pool acquire: {}", e))?;
    run_migrations(&conn)?;
    let _ = conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
    drop(conn);

    POOL.set(pool).map_err(|_| "DB already initialized".to_string())?;
    Ok(())
}

pub fn with_conn<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce(&Connection) -> Result<R, String>,
{
    let pool = POOL.get().ok_or_else(|| "DB not initialized".to_string())?;
    let conn = pool.get().map_err(|e| format!("DB pool acquire: {}", e))?;
    f(&conn)
}

/// Open a single raw SQLite connection for the UserPromptSubmit hook.
///
/// The hook is a fresh OS process per prompt: spinning up an r2d2 pool and
/// running `wal_checkpoint TRUNCATE` on the hot path is pure waste. This
/// opens one connection, sets the minimal pragmas, and runs migrations
/// lazily. No pool, no startup checkpoint.
pub fn open_hook_connection() -> Result<Connection, String> {
    let path = db_path();
    let conn = Connection::open(&path)
        .map_err(|e| format!("open hook DB at {}: {}", path.display(), e))?;
    conn.pragma_update(None, "busy_timeout", "5000")
        .map_err(|e| format!("busy_timeout: {}", e))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| format!("journal_mode: {}", e))?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| format!("synchronous: {}", e))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| format!("foreign_keys: {}", e))?;

    // Run migrations idempotently — short-circuits fast via the schema_version
    // check when already current. Covers the edge case of a hook invocation
    // against a DB that was created by an older build and never touched by a
    // newer GUI/MCP process yet.
    run_migrations(&conn)?;

    Ok(conn)
}

/// Flush WAL to disk before the process exits.
pub fn shutdown() {
    if let Some(pool) = POOL.get() {
        if let Ok(conn) = pool.get() {
            let _ = conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
        }
    }
}

fn build_pool() -> Result<Pool<SqliteConnectionManager>, String> {
    let path = db_path();
    let manager = SqliteConnectionManager::file(&path).with_init(|c| {
        c.pragma_update(None, "busy_timeout", "5000")?;
        c.pragma_update(None, "journal_mode", "WAL")?;
        c.pragma_update(None, "synchronous", "NORMAL")?;
        c.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    });

    Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(|e| format!("Failed to build DB pool at {}: {}", path.display(), e))
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

    if version < 3 {
        apply_migration_v3(conn)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (3)", [])
            .map_err(|e| format!("bump v3: {}", e))?;
    }

    if version < 4 {
        apply_migration_v4(conn)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (4)", [])
            .map_err(|e| format!("bump v4: {}", e))?;
    }

    Ok(())
}

fn apply_migration_v3(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        ALTER TABLE memories ADD COLUMN project TEXT;
        CREATE INDEX idx_memories_project ON memories(project);
        "#,
    )
    .map_err(|e| format!("migration v3: {}", e))
}

// Rebuild the AFTER UPDATE trigger with a WHEN clause so access_count bumps
// (issued by the hook on every prompt) don't re-index FTS rows.
fn apply_migration_v4(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        DROP TRIGGER IF EXISTS memories_au;
        CREATE TRIGGER memories_au AFTER UPDATE ON memories
        WHEN new.title IS NOT old.title
          OR new.description IS NOT old.description
          OR new.content IS NOT old.content
        BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, title, description, content)
            VALUES ('delete', old.rowid, old.title, old.description, old.content);
            INSERT INTO memories_fts(rowid, title, description, content)
            VALUES (new.rowid, new.title, new.description, new.content);
        END;
        "#,
    )
    .map_err(|e| format!("migration v4: {}", e))
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
