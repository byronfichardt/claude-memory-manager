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

/// Claim the lease stored under `key` until `until` (unix seconds), but only if
/// it is free or has expired. Atomic across processes, so one of several app,
/// MCP server and hook processes sharing the DB wins.
pub fn claim_lease(key: &str, now: i64, until: i64) -> Result<bool, String> {
    ensure_table()?;
    with_conn(|conn| claim_lease_on(conn, key, now, until))
}

/// Release a lease early so the next claimant need not wait for it to expire.
pub fn release_lease(key: &str) -> Result<(), String> {
    set(key, "0")
}

fn claim_lease_on(conn: &rusqlite::Connection, key: &str, now: i64, until: i64) -> Result<bool, String> {
    let changed = conn
        .execute(
            r#"INSERT INTO settings (key, value) VALUES (?1, ?3)
               ON CONFLICT(key) DO UPDATE SET value = excluded.value
               WHERE CAST(settings.value AS INTEGER) <= ?2"#,
            params![key, now, until.to_string()],
        )
        .map_err(|e| format!("claim lease {}: {}", key, e))?;
    Ok(changed == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_is_exclusive_until_it_expires() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();

        assert!(claim_lease_on(&conn, "lease", 100, 200).unwrap());
        assert!(!claim_lease_on(&conn, "lease", 150, 250).unwrap());
        assert!(claim_lease_on(&conn, "lease", 200, 300).unwrap());
        assert!(claim_lease_on(&conn, "other", 150, 250).unwrap());
    }
}
