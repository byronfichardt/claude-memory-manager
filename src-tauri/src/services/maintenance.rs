//! Background upkeep: edge prune plus auto-organize.
//!
//! Runs in the tray app and in every MCP server process, because the tray app
//! is often not running while Claude Code sessions are. A DB lease lets one
//! process per interval do the work.

use std::time::Duration;

use tauri::AppHandle;

use crate::services::organizer;
use crate::store::settings;

const CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Lets startup work (migrations, embedding sweep, MCP handshake) finish first.
const STARTUP_DELAY: Duration = Duration::from_secs(60);

const SETTING_TICK_LEASE: &str = "maintenance_tick_until";

/// Slightly under the interval so jitter between processes never skips a window.
const TICK_LEASE_SECS: i64 = CHECK_INTERVAL.as_secs() as i64 - 60;

pub fn spawn(handle: Option<AppHandle>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            tick(handle.clone()).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

async fn tick(handle: Option<AppHandle>) {
    let now = chrono::Utc::now().timestamp();
    if !settings::claim_lease(SETTING_TICK_LEASE, now, now + TICK_LEASE_SECS).unwrap_or(false) {
        return;
    }

    match tauri::async_runtime::spawn_blocking(organizer::prune_weak_edges).await {
        Ok(Ok(n)) if n > 0 => eprintln!("maintenance: pruned {} edges", n),
        Ok(Err(e)) => eprintln!("maintenance: prune failed: {}", e),
        _ => {}
    }

    if let Some(report) = organizer::maybe_auto_organize(handle).await {
        eprintln!(
            "maintenance: classified {}, merged {}, pruned {} edges",
            report.classified_count, report.merged_count, report.pruned_edges
        );
    }
}
