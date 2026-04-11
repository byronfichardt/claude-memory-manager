use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::with_conn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Topic {
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub created_at: i64,
    pub memory_count: i64,
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Insert a topic if not present (upsert by name).
pub fn ensure(name: &str, description: Option<&str>, color: Option<&str>) -> Result<(), String> {
    with_conn(|conn| {
        let now = now_ts();
        conn.execute(
            r#"INSERT INTO topics (name, description, color, created_at)
               VALUES (?1, ?2, ?3, ?4)
               ON CONFLICT(name) DO UPDATE SET
                 description = COALESCE(excluded.description, topics.description),
                 color = COALESCE(excluded.color, topics.color)"#,
            params![name, description, color, now],
        )
        .map_err(|e| format!("ensure topic: {}", e))?;
        Ok(())
    })
}

/// List all topics with their memory counts.
pub fn list_all() -> Result<Vec<Topic>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                r#"SELECT t.name, t.description, t.color, t.created_at,
                          COUNT(m.id) as memory_count
                   FROM topics t
                   LEFT JOIN memories m ON m.topic = t.name
                   GROUP BY t.name
                   ORDER BY memory_count DESC, t.name ASC"#,
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                Ok(Topic {
                    name: row.get("name")?,
                    description: row.get("description")?,
                    color: row.get("color")?,
                    created_at: row.get("created_at")?,
                    memory_count: row.get("memory_count")?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

/// Delete a topic only if no memories reference it. Safe operation.
pub fn delete_empty(name: &str) -> Result<(), String> {
    with_conn(|conn| {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE topic = ?1",
                params![name],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if count == 0 {
            conn.execute("DELETE FROM topics WHERE name = ?1", params![name])
                .map_err(|e| format!("delete topic: {}", e))?;
        }
        Ok(())
    })
}
