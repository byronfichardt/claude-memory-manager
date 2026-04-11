//! End-to-end smoke test for the store. Run with:
//!   cargo test --lib store::smoke_test -- --nocapture

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    /// Mirror of apply_migration_v1 for isolated testing.
    fn setup_schema(conn: &Connection) {
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
    }

    #[test]
    fn fts_search_returns_matching_rows() {
        let conn = Connection::open_in_memory().unwrap();
        setup_schema(&conn);

        conn.execute(
            r#"INSERT INTO memories
               (id, title, description, content, content_hash, created_at, updated_at)
               VALUES ('1', 'Docker deployment', 'How we deploy', 'We use docker compose for staging and k8s for prod', 'h1', 0, 0),
                      ('2', 'Testing strategy', 'How we test', 'Use cargo test for rust and vitest for vue', 'h2', 0, 0)"#,
            [],
        )
        .unwrap();

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
}
