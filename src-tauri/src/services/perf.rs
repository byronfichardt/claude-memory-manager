//! Per-invocation timing log for the hook and MCP tools.
//!
//! Each finished `Timer` appends one JSON line to `<data_dir>/perf.log`, with
//! per-phase durations and how long the call spent waiting on SQLite locks.
//! The file rotates to `perf.log.1` past `MAX_LOG_BYTES`. Logging is
//! best-effort and never fails the caller.

use std::io::Write;
use std::time::Instant;

use serde_json::{json, Map, Value};

use crate::store;

const LOG_FILE: &str = "perf.log";
const ROTATED_LOG_FILE: &str = "perf.log.1";
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub struct Timer {
    started: Instant,
    last_mark: Instant,
    lock_wait_start_ms: u64,
    phases: Map<String, Value>,
}

impl Timer {
    pub fn start() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_mark: now,
            lock_wait_start_ms: store::lock_wait_ms(),
            phases: Map::new(),
        }
    }

    /// Record the time since the previous mark (or start) under `phase`.
    pub fn mark(&mut self, phase: &str) {
        let now = Instant::now();
        self.phases
            .insert(phase.to_string(), json!(millis(now - self.last_mark)));
        self.last_mark = now;
    }

    pub fn finish(self, kind: &str, name: &str, extra: Value) {
        let mut entry = json!({
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "pid": std::process::id(),
            "kind": kind,
            "name": name,
            "total_ms": millis(self.started.elapsed()),
            "lock_wait_ms": store::lock_wait_ms().saturating_sub(self.lock_wait_start_ms),
            "phases": self.phases,
        });
        if let (Some(entry), Value::Object(extra)) = (entry.as_object_mut(), extra) {
            entry.extend(extra);
        }
        append(&entry);
    }
}

pub fn file_size(path: &std::path::Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

fn millis(d: std::time::Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

fn append(entry: &Value) {
    let dir = store::data_dir();
    let path = dir.join(LOG_FILE);
    if file_size(&path).is_some_and(|len| len > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, dir.join(ROTATED_LOG_FILE));
    }
    // One write per line: concurrent hooks append to the same file, and a
    // formatted write is split into many small writes that interleave.
    let line = format!("{}\n", entry);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}
