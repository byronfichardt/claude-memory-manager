//! End-to-end smoke test for the store. Run with:
//!   cargo test --lib store::smoke_test -- --nocapture

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    /// Mirror of apply_migration_v1 + v2 for isolated testing.
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

            -- v2: memory_edges
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
        .unwrap();
    }

    fn insert_memory(conn: &Connection, id: &str, title: &str, content: &str) {
        conn.execute(
            r#"INSERT INTO memories
               (id, title, description, content, content_hash, created_at, updated_at)
               VALUES (?1, ?2, '', ?3, ?4, 0, 0)"#,
            rusqlite::params![id, title, content, format!("hash_{}", id)],
        )
        .unwrap();
    }

    #[test]
    fn fts_search_returns_matching_rows() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);

        insert_memory(&conn, "1", "Docker deployment", "We use docker compose for staging and k8s for prod");
        insert_memory(&conn, "2", "Testing strategy", "Use cargo test for rust and vitest for vue");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        // FTS search
        let mut stmt = conn
            .prepare(
                "SELECT m.id FROM memories_fts JOIN memories m ON m.rowid = memories_fts.rowid WHERE memories_fts MATCH ?1",
            )
            .unwrap();
        let rows: Vec<String> = stmt
            .query_map(["docker*"], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows, vec!["1".to_string()]);

        let rows: Vec<String> = stmt
            .query_map(["cargo*"], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows, vec!["2".to_string()]);
    }

    #[test]
    fn edge_insert_and_get_neighbors() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);

        insert_memory(&conn, "a", "Memory A", "Content A");
        insert_memory(&conn, "b", "Memory B", "Content B");
        insert_memory(&conn, "c", "Memory C", "Content C");

        // Insert edges
        conn.execute(
            r#"INSERT INTO memory_edges (source_id, target_id, edge_type, weight, source_origin, created_at, updated_at)
               VALUES ('a', 'b', 'relates-to', 0.5, 'ai_discovered', 0, 0)"#,
            [],
        ).unwrap();
        conn.execute(
            r#"INSERT INTO memory_edges (source_id, target_id, edge_type, weight, source_origin, created_at, updated_at)
               VALUES ('a', 'c', 'depends-on', 0.8, 'ai_discovered', 0, 0)"#,
            [],
        ).unwrap();

        // Get neighbors of 'a' — should find both edges
        let mut stmt = conn
            .prepare("SELECT * FROM memory_edges WHERE source_id = ?1 OR target_id = ?1")
            .unwrap();
        let count = stmt
            .query_map(["a"], |_| Ok(()))
            .unwrap()
            .count();
        assert_eq!(count, 2);

        // Get neighbors of 'b' — should find one edge
        let count = stmt
            .query_map(["b"], |_| Ok(()))
            .unwrap()
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn edge_upsert_takes_max_weight() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);

        insert_memory(&conn, "a", "Memory A", "Content A");
        insert_memory(&conn, "b", "Memory B", "Content B");

        conn.execute(
            r#"INSERT INTO memory_edges (source_id, target_id, edge_type, weight, source_origin, created_at, updated_at)
               VALUES ('a', 'b', 'relates-to', 0.3, 'co_access', 0, 0)"#,
            [],
        ).unwrap();

        // Upsert with higher weight
        conn.execute(
            r#"INSERT INTO memory_edges (source_id, target_id, edge_type, weight, source_origin, created_at, updated_at)
               VALUES ('a', 'b', 'relates-to', 0.7, 'ai_discovered', 1, 1)
               ON CONFLICT(source_id, target_id, edge_type) DO UPDATE SET
                   weight = MAX(memory_edges.weight, excluded.weight),
                   updated_at = excluded.updated_at"#,
            [],
        ).unwrap();

        let weight: f64 = conn
            .query_row(
                "SELECT weight FROM memory_edges WHERE source_id = 'a' AND target_id = 'b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((weight - 0.7).abs() < f64::EPSILON);

        // Upsert with lower weight — should keep 0.7
        conn.execute(
            r#"INSERT INTO memory_edges (source_id, target_id, edge_type, weight, source_origin, created_at, updated_at)
               VALUES ('a', 'b', 'relates-to', 0.2, 'co_access', 2, 2)
               ON CONFLICT(source_id, target_id, edge_type) DO UPDATE SET
                   weight = MAX(memory_edges.weight, excluded.weight),
                   updated_at = excluded.updated_at"#,
            [],
        ).unwrap();

        let weight: f64 = conn
            .query_row(
                "SELECT weight FROM memory_edges WHERE source_id = 'a' AND target_id = 'b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!((weight - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn edge_cascade_on_memory_delete() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);

        insert_memory(&conn, "a", "Memory A", "Content A");
        insert_memory(&conn, "b", "Memory B", "Content B");

        conn.execute(
            r#"INSERT INTO memory_edges (source_id, target_id, edge_type, weight, source_origin, created_at, updated_at)
               VALUES ('a', 'b', 'relates-to', 0.5, 'ai_discovered', 0, 0)"#,
            [],
        ).unwrap();

        let edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edge_count, 1);

        // Delete memory 'a' — edge should cascade
        conn.execute("DELETE FROM memories WHERE id = 'a'", []).unwrap();

        let edge_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_edges", [], |r| r.get(0))
            .unwrap();
        assert_eq!(edge_count, 0);
    }
}
