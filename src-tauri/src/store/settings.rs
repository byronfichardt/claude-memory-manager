//! Simple key/value settings table for app preferences.

use rusqlite::params;

use super::with_conn;

pub fn ensure_table() -> Result<(), String> {
    with_conn(|conn| {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )
        .map_err(|e| format!("settings table: {}", e))
    })
}

pub fn get(key: &str, default: &str) -> Result<String, String> {
    ensure_table()?;
    with_conn(|conn| {
        let result: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .ok();
        Ok(result.unwrap_or_else(|| default.to_string()))
    })
}

pub fn set(key: &str, value: &str) -> Result<(), String> {
    ensure_table()?;
    with_conn(|conn| {
        conn.execute(
            r#"INSERT INTO settings (key, value) VALUES (?1, ?2)
               ON CONFLICT(key) DO UPDATE SET value = excluded.value"#,
            params![key, value],
        )
        .map_err(|e| format!("upsert setting: {}", e))?;
        Ok(())
    })
}

pub fn get_bool(key: &str, default: bool) -> Result<bool, String> {
    let val = get(key, if default { "true" } else { "false" })?;
    Ok(val == "true")
}

pub fn set_bool(key: &str, value: bool) -> Result<(), String> {
    set(key, if value { "true" } else { "false" })
}
