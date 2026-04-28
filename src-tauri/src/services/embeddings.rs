//! Semantic search via local embeddings (nomic-embed-text-v1.5, 768-dim).
//!
//! Opt-in only — the model (~275 MB) is downloaded on first enable and cached
//! under <data_dir>/models/. Until the model is ready or while memories are
//! still being indexed, all callers fall back silently to FTS5.
//!
//! Architecture:
//!   - GUI process owns the model (OnceLock<Mutex<Option<TextEmbedding>>>).
//!   - A background sweep thread generates embeddings for un-indexed memories.
//!   - Hook and MCP server skip vector search (FTS5 only) — they can't load
//!     the model without an unacceptable latency hit on every prompt.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::store::{self, settings, with_conn};

// ── Constants ────────────────────────────────────────────────────────────────

const SETTING_ENABLED: &str = "semantic_search_enabled";
pub const EMBED_DIM: usize = 768;
const SWEEP_BATCH: usize = 16;

// ── Statics ──────────────────────────────────────────────────────────────────

static MODEL: OnceLock<Mutex<Option<TextEmbedding>>> = OnceLock::new();
static IS_DOWNLOADING: AtomicBool = AtomicBool::new(false);
static IS_SWEEPING: AtomicBool = AtomicBool::new(false);
static SWEEP_INDEXED: OnceLock<Mutex<u64>> = OnceLock::new();
static SWEEP_TOTAL: OnceLock<Mutex<u64>> = OnceLock::new();

fn model_slot() -> &'static Mutex<Option<TextEmbedding>> {
    MODEL.get_or_init(|| Mutex::new(None))
}

fn sweep_indexed() -> &'static Mutex<u64> {
    SWEEP_INDEXED.get_or_init(|| Mutex::new(0))
}

fn sweep_total() -> &'static Mutex<u64> {
    SWEEP_TOTAL.get_or_init(|| Mutex::new(0))
}

// ── Public types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingStatus {
    pub enabled: bool,
    pub is_downloading: bool,
    pub model_ready: bool,
    pub indexed_count: u64,
    pub total_count: u64,
    pub is_sweeping: bool,
}

// ── Queries ───────────────────────────────────────────────────────────────────

pub fn is_enabled() -> bool {
    settings::get_bool(SETTING_ENABLED, false).unwrap_or(false)
}

pub fn is_model_ready() -> bool {
    MODEL
        .get()
        .and_then(|m| m.lock().ok())
        .map(|g| g.is_some())
        .unwrap_or(false)
}

pub fn get_status() -> EmbeddingStatus {
    let enabled = is_enabled();
    let is_downloading = IS_DOWNLOADING.load(Ordering::Relaxed);
    let model_ready = is_model_ready();
    let is_sweeping = IS_SWEEPING.load(Ordering::Relaxed);

    let mut indexed_count = sweep_indexed().lock().map(|g| *g).unwrap_or(0);
    let mut total_count = sweep_total().lock().map(|g| *g).unwrap_or(0);

    // If no sweep in progress yet, derive counts directly from the DB.
    if !is_sweeping && !is_downloading && model_ready && total_count == 0 {
        indexed_count = count_indexed().unwrap_or(0) as u64;
        total_count = store::memories::count().unwrap_or(0) as u64;
    }

    EmbeddingStatus {
        enabled,
        is_downloading,
        model_ready,
        indexed_count,
        total_count,
        is_sweeping,
    }
}

fn count_indexed() -> Result<i64, String> {
    with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM vec_memories", [], |r| r.get(0))
            .map_err(|e| e.to_string())
    })
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

/// Enable semantic search: persist setting, then start background init + sweep.
pub fn enable() -> Result<(), String> {
    settings::set_bool(SETTING_ENABLED, true)?;
    spawn_init_and_sweep();
    Ok(())
}

/// Disable semantic search: persist setting, release model from RAM.
pub fn disable() -> Result<(), String> {
    settings::set_bool(SETTING_ENABLED, false)?;
    if let Ok(mut g) = model_slot().lock() {
        *g = None;
    }
    Ok(())
}

/// Called at app startup — loads the model in background if previously enabled.
pub fn maybe_init_on_startup() {
    if is_enabled() {
        spawn_init_and_sweep();
    }
}

fn spawn_init_and_sweep() {
    std::thread::spawn(|| {
        if let Err(e) = init_model_blocking() {
            eprintln!("[embeddings] model init failed: {}", e);
            IS_DOWNLOADING.store(false, Ordering::Relaxed);
            return;
        }
        run_sweep_blocking();
    });
}

fn model_cache_dir() -> std::path::PathBuf {
    store::data_dir().join("models")
}

fn init_model_blocking() -> Result<(), String> {
    if is_model_ready() {
        return Ok(());
    }

    IS_DOWNLOADING.store(true, Ordering::Relaxed);

    let cache_dir = model_cache_dir();
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("create model cache dir: {}", e))?;

    let model = TextEmbedding::try_new(
        InitOptions::new(EmbeddingModel::NomicEmbedTextV15)
            .with_cache_dir(cache_dir)
            .with_show_download_progress(false),
    )
    .map_err(|e| format!("init embedding model: {}", e))?;

    if let Ok(mut g) = model_slot().lock() {
        *g = Some(model);
    }

    IS_DOWNLOADING.store(false, Ordering::Relaxed);
    Ok(())
}

// ── Embedding generation ──────────────────────────────────────────────────────

/// Generate embeddings for a slice of texts. Returns None if model not ready.
pub fn embed(texts: &[&str]) -> Option<Vec<Vec<f32>>> {
    let guard = MODEL.get()?.lock().ok()?;
    let model = guard.as_ref()?;
    model.embed(texts.to_vec(), None).ok()
}

pub fn embed_single(text: &str) -> Option<Vec<f32>> {
    embed(&[text])?.into_iter().next()
}

// ── Background sweep ──────────────────────────────────────────────────────────

pub fn trigger_sweep() {
    if IS_SWEEPING.load(Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(run_sweep_blocking);
}

fn run_sweep_blocking() {
    if IS_SWEEPING.swap(true, Ordering::Relaxed) {
        return;
    }

    let total = store::memories::count().unwrap_or(0) as u64;
    if let Ok(mut g) = sweep_total().lock() {
        *g = total;
    }
    if let Ok(mut g) = sweep_indexed().lock() {
        *g = count_indexed().unwrap_or(0) as u64;
    }

    loop {
        let batch = match fetch_unindexed_batch(SWEEP_BATCH) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[embeddings] fetch_unindexed_batch error: {}", e);
                break;
            }
        };

        if batch.is_empty() {
            break;
        }

        let texts: Vec<&str> = batch.iter().map(|(_, t)| t.as_str()).collect();
        let embeddings = match embed(&texts) {
            Some(e) => e,
            None => break, // model dropped or not ready
        };

        let pairs: Vec<(&str, &Vec<f32>)> = batch
            .iter()
            .map(|(id, _)| id.as_str())
            .zip(embeddings.iter())
            .collect();

        if let Err(e) = store_embeddings(&pairs) {
            eprintln!("[embeddings] store_embeddings error: {}", e);
            break;
        }

        let now_indexed = count_indexed().unwrap_or(0) as u64;
        if let Ok(mut g) = sweep_indexed().lock() {
            *g = now_indexed;
        }
    }

    IS_SWEEPING.store(false, Ordering::Relaxed);
}

fn fetch_unindexed_batch(limit: usize) -> Result<Vec<(String, String)>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.title || ' ' || m.content
                 FROM memories m
                 WHERE m.id NOT IN (SELECT memory_id FROM vec_memories)
                 LIMIT ?1",
            )
            .map_err(|e| format!("prepare fetch_unindexed: {}", e))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("query fetch_unindexed: {}", e))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

fn store_embeddings(pairs: &[(&str, &Vec<f32>)]) -> Result<(), String> {
    with_conn(|conn| {
        let tx = conn.unchecked_transaction().map_err(|e| format!("begin tx: {}", e))?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO vec_memories(memory_id, embedding) VALUES (?1, ?2)",
                )
                .map_err(|e| format!("prepare store_embedding: {}", e))?;

            for (id, embedding) in pairs {
                let bytes = f32_slice_to_bytes(embedding);
                stmt.execute(params![id, bytes])
                    .map_err(|e| format!("insert embedding for {}: {}", id, e))?;
            }
        }
        tx.commit().map_err(|e| format!("commit embeddings: {}", e))
    })
}

/// Queue a single memory for indexing after it's created/updated.
/// Fire-and-forget — errors are swallowed since FTS5 fallback still works.
pub fn queue_memory(id: &str, text: &str) {
    if !is_model_ready() {
        return;
    }
    let id = id.to_string();
    let text = text.to_string();
    std::thread::spawn(move || {
        if let Some(embedding) = embed_single(&text) {
            let _ = store_embeddings(&[(&id, &embedding)]);
        }
    });
}

// ── Vector search ─────────────────────────────────────────────────────────────

/// KNN search in the vec_memories table. Returns (memory_id, distance) pairs.
/// Returns None if the model isn't ready (caller should fall back to FTS5).
pub fn vector_search(query: &str, limit: usize) -> Option<Vec<(String, f64)>> {
    let embedding = embed_single(query)?;
    let bytes = f32_slice_to_bytes(&embedding);

    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT memory_id, distance
                 FROM vec_memories
                 WHERE embedding MATCH ?1
                 ORDER BY distance
                 LIMIT ?2",
            )
            .map_err(|e| format!("prepare vector_search: {}", e))?;

        let rows = stmt
            .query_map(params![bytes, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(|e| format!("query vector_search: {}", e))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
    .ok()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn f32_slice_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|&f| f.to_le_bytes()).collect()
}
