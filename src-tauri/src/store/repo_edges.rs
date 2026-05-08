use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::with_conn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEdge {
    pub id: i64,
    pub source_repo: String,
    pub target_repo: String,
    pub relationship_type: String,
    pub evidence: String,
    pub namespace: String,
    pub weight: f64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoGraph {
    pub nodes: Vec<String>,
    pub edges: Vec<RepoEdge>,
    pub namespaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProposal {
    pub source_repo: String,
    pub target_repo: String,
    pub relationship_type: String,
    pub evidence: String,
}

fn row_to_edge(row: &Row) -> rusqlite::Result<RepoEdge> {
    Ok(RepoEdge {
        id: row.get("id")?,
        source_repo: row.get("source_repo")?,
        target_repo: row.get("target_repo")?,
        relationship_type: row.get("relationship_type")?,
        evidence: row.get("evidence")?,
        namespace: row.get("namespace")?,
        weight: row.get("weight")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn upsert(
    source_repo: &str,
    target_repo: &str,
    relationship_type: &str,
    evidence: &str,
    namespace: &str,
) -> Result<RepoEdge, String> {
    with_conn(|conn| upsert_with_conn(conn, source_repo, target_repo, relationship_type, evidence, namespace))
}

pub fn upsert_with_conn(
    conn: &Connection,
    source_repo: &str,
    target_repo: &str,
    relationship_type: &str,
    evidence: &str,
    namespace: &str,
) -> Result<RepoEdge, String> {
    let now = now_ts();

    conn.execute(
        r#"INSERT INTO repo_edges (source_repo, target_repo, relationship_type, evidence, namespace, weight, created_at, updated_at)
           VALUES (?1, ?2, ?3, ?4, ?5, 1.0, ?6, ?6)
           ON CONFLICT(source_repo, target_repo, relationship_type) DO UPDATE SET
               evidence = CASE
                   WHEN evidence = '' THEN excluded.evidence
                   WHEN excluded.evidence = '' THEN evidence
                   WHEN instr(evidence, excluded.evidence) > 0 THEN evidence
                   ELSE evidence || '; ' || excluded.evidence
               END,
               namespace = excluded.namespace,
               updated_at = ?6"#,
        params![source_repo, target_repo, relationship_type, evidence, namespace, now],
    )
    .map_err(|e| format!("upsert repo_edge: {}", e))?;

    conn.query_row(
        "SELECT * FROM repo_edges WHERE source_repo = ?1 AND target_repo = ?2 AND relationship_type = ?3",
        params![source_repo, target_repo, relationship_type],
        row_to_edge,
    )
    .map_err(|e| format!("fetch upserted repo_edge: {}", e))
}

pub fn list(filter_repo: Option<&str>, filter_namespace: Option<&str>) -> Result<Vec<RepoEdge>, String> {
    with_conn(|conn| {
        match (filter_repo, filter_namespace) {
            (None, None) => {
                let mut stmt = conn
                    .prepare("SELECT * FROM repo_edges ORDER BY updated_at DESC")
                    .map_err(|e| format!("prepare: {}", e))?;
                let rows = stmt.query_map([], row_to_edge).map_err(|e| format!("query: {}", e))?;
                collect(rows)
            }
            (Some(repo), None) => {
                let mut stmt = conn
                    .prepare("SELECT * FROM repo_edges WHERE source_repo = ?1 OR target_repo = ?1 ORDER BY updated_at DESC")
                    .map_err(|e| format!("prepare: {}", e))?;
                let rows = stmt.query_map(params![repo], row_to_edge).map_err(|e| format!("query: {}", e))?;
                collect(rows)
            }
            (None, Some(ns)) => {
                let mut stmt = conn
                    .prepare("SELECT * FROM repo_edges WHERE namespace = ?1 ORDER BY updated_at DESC")
                    .map_err(|e| format!("prepare: {}", e))?;
                let rows = stmt.query_map(params![ns], row_to_edge).map_err(|e| format!("query: {}", e))?;
                collect(rows)
            }
            (Some(repo), Some(ns)) => {
                let mut stmt = conn
                    .prepare("SELECT * FROM repo_edges WHERE (source_repo = ?1 OR target_repo = ?1) AND namespace = ?2 ORDER BY updated_at DESC")
                    .map_err(|e| format!("prepare: {}", e))?;
                let rows = stmt.query_map(params![repo, ns], row_to_edge).map_err(|e| format!("query: {}", e))?;
                collect(rows)
            }
        }
    })
}

pub fn list_namespaces() -> Result<Vec<String>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT DISTINCT namespace FROM repo_edges ORDER BY namespace")
            .map_err(|e| format!("prepare list_namespaces: {}", e))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| format!("query list_namespaces: {}", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    })
}

pub fn full_graph(filter_namespace: Option<&str>) -> Result<RepoGraph, String> {
    with_conn(|conn| {
        let edges: Vec<RepoEdge> = match filter_namespace {
            None => {
                let mut stmt = conn
                    .prepare("SELECT * FROM repo_edges ORDER BY updated_at DESC")
                    .map_err(|e| format!("prepare full_graph: {}", e))?;
                let rows = stmt.query_map([], row_to_edge).map_err(|e| format!("query full_graph: {}", e))?;
                collect(rows)?
            }
            Some(ns) => {
                let mut stmt = conn
                    .prepare("SELECT * FROM repo_edges WHERE namespace = ?1 ORDER BY updated_at DESC")
                    .map_err(|e| format!("prepare full_graph_ns: {}", e))?;
                let rows = stmt.query_map(params![ns], row_to_edge).map_err(|e| format!("query full_graph_ns: {}", e))?;
                collect(rows)?
            }
        };

        let mut nodes: Vec<String> = Vec::new();
        for edge in &edges {
            if !nodes.contains(&edge.source_repo) {
                nodes.push(edge.source_repo.clone());
            }
            if !nodes.contains(&edge.target_repo) {
                nodes.push(edge.target_repo.clone());
            }
        }
        nodes.sort();

        let namespaces: Vec<String> = {
            let mut ns_stmt = conn
                .prepare("SELECT DISTINCT namespace FROM repo_edges ORDER BY namespace")
                .map_err(|e| format!("prepare namespaces: {}", e))?;
            let rows = ns_stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| format!("query namespaces: {}", e))?;
            let mut out = Vec::new();
            for r in rows { out.push(r.map_err(|e| e.to_string())?); }
            out
        };

        Ok(RepoGraph { nodes, edges, namespaces })
    })
}

/// Get repos that the current repo depends on (outgoing edges).
pub fn dependencies_of(repo: &str) -> Result<Vec<RepoEdge>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT * FROM repo_edges WHERE source_repo = ?1 ORDER BY weight DESC")
            .map_err(|e| format!("prepare dependencies_of: {}", e))?;
        let raw: Vec<rusqlite::Result<RepoEdge>> = stmt
            .query_map(params![repo], row_to_edge)
            .map_err(|e| format!("query dependencies_of: {}", e))?
            .collect();
        raw.into_iter().map(|r| r.map_err(|e| e.to_string())).collect()
    })
}

pub fn delete(id: i64) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute("DELETE FROM repo_edges WHERE id = ?1", params![id])
            .map_err(|e| format!("delete repo_edge: {}", e))?;
        Ok(())
    })
}

pub fn count() -> Result<i64, String> {
    with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM repo_edges", [], |r| r.get(0))
            .map_err(|e| e.to_string())
    })
}

/// Scan a directory for git repos and propose relationships based on env vars and composer/package.json.
pub fn scan_directory(root: &str) -> Result<Vec<ScanProposal>, String> {
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        return Err(format!("Not a directory: {}", root));
    }

    // Find all immediate subdirectories that are git repos
    let repo_dirs: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(root_path)
        .map_err(|e| format!("read_dir: {}", e))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() && path.join(".git").exists() {
                let name = path.file_name()?.to_string_lossy().to_string();
                Some((name, path))
            } else {
                None
            }
        })
        .collect();

    let repo_names: Vec<String> = repo_dirs.iter().map(|(n, _)| n.clone()).collect();
    let mut proposals: Vec<ScanProposal> = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    for (repo_name, repo_path) in &repo_dirs {
        // Scan env files for URL variables.
        // Includes both "template" files (Laravel/PHP convention) and real env files
        // that frontend projects (Vite, Next.js, CRA) actually commit or leave in the repo.
        for env_file in &[
            ".env.example", ".env.sample", ".env.local.example", ".env.testing",
            ".env", ".env.local", ".env.development", ".env.production", ".env.staging",
        ] {
            let env_path = repo_path.join(env_file);
            if let Ok(content) = std::fs::read_to_string(&env_path) {
                for line in content.lines() {
                    let line = line.trim();
                    if line.starts_with('#') || !line.contains('=') {
                        continue;
                    }
                    let key = line.split('=').next().unwrap_or("").trim();
                    if let Some(matched_repo) = match_url_var(key, &repo_names, repo_name) {
                        let dedup_key = (repo_name.clone(), matched_repo.clone(), "calls".to_string());
                        if seen.insert(dedup_key) {
                            proposals.push(ScanProposal {
                                source_repo: repo_name.clone(),
                                target_repo: matched_repo,
                                relationship_type: "calls".to_string(),
                                evidence: format!("{}: {}", env_file, key),
                            });
                        }
                    }
                }
            }
        }

        // Scan composer.json for internal package dependencies
        let composer_path = repo_path.join("composer.json");
        if let Ok(content) = std::fs::read_to_string(&composer_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                let mut all_deps: HashMap<String, ()> = HashMap::new();
                for dep_key in &["require", "require-dev"] {
                    if let Some(deps) = json.get(dep_key).and_then(|v| v.as_object()) {
                        for k in deps.keys() {
                            all_deps.insert(k.clone(), ());
                        }
                    }
                }
                for pkg_name in all_deps.keys() {
                    let pkg_short = pkg_name.split('/').last().unwrap_or(pkg_name);
                    if let Some(matched_repo) = match_package(pkg_short, &repo_names, repo_name) {
                        let dedup_key = (repo_name.clone(), matched_repo.clone(), "imports".to_string());
                        if seen.insert(dedup_key) {
                            proposals.push(ScanProposal {
                                source_repo: repo_name.clone(),
                                target_repo: matched_repo,
                                relationship_type: "imports".to_string(),
                                evidence: format!("composer.json: \"{}\"", pkg_name),
                            });
                        }
                    }
                }
            }
        }

        // Scan package.json for internal package dependencies
        let pkg_path = repo_path.join("package.json");
        if let Ok(content) = std::fs::read_to_string(&pkg_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                let mut all_deps: HashMap<String, ()> = HashMap::new();
                for dep_key in &["dependencies", "devDependencies"] {
                    if let Some(deps) = json.get(dep_key).and_then(|v| v.as_object()) {
                        for k in deps.keys() {
                            all_deps.insert(k.clone(), ());
                        }
                    }
                }
                for pkg_name in all_deps.keys() {
                    let pkg_short = pkg_name.split('/').last().unwrap_or(pkg_name);
                    if let Some(matched_repo) = match_package(pkg_short, &repo_names, repo_name) {
                        let dedup_key = (repo_name.clone(), matched_repo.clone(), "imports".to_string());
                        if seen.insert(dedup_key) {
                            proposals.push(ScanProposal {
                                source_repo: repo_name.clone(),
                                target_repo: matched_repo,
                                relationship_type: "imports".to_string(),
                                evidence: format!("package.json: \"{}\"", pkg_name),
                            });
                        }
                    }
                }
            }
        }
    }

    proposals.sort_by(|a, b| a.source_repo.cmp(&b.source_repo).then(a.target_repo.cmp(&b.target_repo)));
    Ok(proposals)
}

fn normalize_env_var(var: &str) -> String {
    // Strip known framework prefixes first so VITE_API_URL → API_URL → "api"
    // rather than VITE_API_URL → "vite-api".
    const PREFIXES: &[&str] = &[
        "NEXT_PUBLIC_",
        "VITE_",
        "REACT_APP_",
        "NUXT_PUBLIC_",
        "EXPO_PUBLIC_",
        "PUBLIC_",
    ];
    let upper = var.to_uppercase();
    let stripped = {
        let mut s = upper.as_str();
        for prefix in PREFIXES {
            if s.starts_with(prefix) {
                s = &s[prefix.len()..];
                break;
            }
        }
        s.to_string()
    };

    let suffixes = [
        "_BASE_URL", "_BASEURL", "_SERVICE_URL", "_API_URL",
        "_API_BASEURL", "_ENDPOINT", "_HOST", "_URL",
    ];
    let mut s = stripped;
    for suffix in &suffixes {
        if s.ends_with(suffix) {
            s = s[..s.len() - suffix.len()].to_string();
            break;
        }
    }
    s.to_lowercase().replace('_', "-")
}

fn match_url_var(var: &str, repo_names: &[String], exclude: &str) -> Option<String> {
    let upper = var.to_uppercase();
    if !upper.ends_with("_URL") && !upper.ends_with("_BASEURL") && !upper.ends_with("_BASE_URL")
        && !upper.ends_with("_SERVICE_URL") && !upper.ends_with("_API_URL")
        && !upper.ends_with("_ENDPOINT") && !upper.ends_with("_HOST")
    {
        return None;
    }

    let normalized = normalize_env_var(var);
    if normalized.len() < 3 {
        return None;
    }

    for repo in repo_names {
        if repo == exclude {
            continue;
        }
        if normalized == *repo {
            return Some(repo.clone());
        }
        if repo.starts_with(&normalized) && normalized.len() >= 4 {
            return Some(repo.clone());
        }
        if normalized.starts_with(repo.as_str()) && repo.len() >= 4 {
            return Some(repo.clone());
        }
    }
    None
}

fn match_package(pkg_short: &str, repo_names: &[String], exclude: &str) -> Option<String> {
    let pkg_lower = pkg_short.to_lowercase();
    for repo in repo_names {
        if repo == exclude {
            continue;
        }
        if pkg_lower == *repo {
            return Some(repo.clone());
        }
    }
    None
}

fn collect(
    rows: impl Iterator<Item = rusqlite::Result<RepoEdge>>,
) -> Result<Vec<RepoEdge>, String> {
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}
