//! Minimal MCP stdio server implementation.
//!
//! Protocol: JSON-RPC 2.0 over stdin/stdout with newline-delimited framing.
//! Each message is one line of JSON terminated by `\n`.
//!
//! Implements only the minimum: initialize, tools/list, tools/call.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::services::project;
use crate::store::{edges, memories, repo_edges};

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "claude-memory-manager";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Resolved at MCP server startup — the git root of whatever directory Claude Code
/// spawned the server from, if any. Fallback for when the hook hasn't written a
/// fresh pointer (e.g. programmatic MCP clients that don't run a UserPromptSubmit
/// hook).
static SERVER_PROJECT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Best guess at the project the user is currently working on.
/// Priority:
///   1. Active-project pointer written by the UserPromptSubmit hook — this is
///      the most accurate signal because the hook has transcript access.
///   2. The git root of wherever the MCP server was spawned from (startup cwd).
///   3. None.
fn detected_project() -> Option<PathBuf> {
    if let Some(p) = project::read_active_project() {
        return Some(p);
    }
    SERVER_PROJECT.get().and_then(|o| o.clone())
}

#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// Run the stdio MCP server. Blocks until stdin is closed.
pub fn run() -> io::Result<()> {
    // Capture the project the MCP server was spawned from. Used as a fallback
    // "current project" in tool calls when Claude doesn't pass one explicitly.
    let startup_project = std::env::current_dir()
        .ok()
        .and_then(|cwd| project::resolve_project(&cwd));
    let _ = SERVER_PROJECT.set(startup_project);

    // Initialize the store lazily — fast with WAL SQLite
    if let Err(e) = crate::store::init() {
        eprintln!("[mcp] failed to init store: {}", e);
        return Err(io::Error::other(e));
    }

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();
    let mut reader = stdin.lock();
    let mut line = String::new();

    eprintln!("[mcp] server ready, waiting for requests");

    loop {
        line.clear();
        let n = match reader.read_line(&mut line) {
            Ok(0) => {
                eprintln!("[mcp] stdin closed, exiting");
                return Ok(());
            }
            Ok(n) => n,
            Err(e) => {
                eprintln!("[mcp] read error: {}", e);
                return Err(e);
            }
        };

        if n == 0 || line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[mcp] parse error: {} (line: {})", e, line.trim());
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            eprintln!("[mcp] bad jsonrpc version: {}", request.jsonrpc);
            continue;
        }

        // Notifications (no id) don't need responses
        let is_notification = request.id.is_none();

        let result = dispatch(&request.method, request.params.unwrap_or(Value::Null));

        if is_notification {
            continue;
        }

        let response = match result {
            Ok(value) => JsonRpcResponse {
                jsonrpc: "2.0",
                id: request.id.unwrap_or(Value::Null),
                result: Some(value),
                error: None,
            },
            Err((code, message)) => JsonRpcResponse {
                jsonrpc: "2.0",
                id: request.id.unwrap_or(Value::Null),
                result: None,
                error: Some(JsonRpcError { code, message }),
            },
        };

        let response_json = serde_json::to_string(&response).unwrap();
        writeln!(stdout_lock, "{}", response_json)?;
        stdout_lock.flush()?;
    }
}

fn dispatch(method: &str, params: Value) -> Result<Value, (i32, String)> {
    match method {
        "initialize" => Ok(handle_initialize()),
        "notifications/initialized" => Ok(Value::Null),
        "tools/list" => Ok(handle_tools_list()),
        "tools/call" => handle_tools_call(params),
        "ping" => Ok(json!({})),
        _ => Err((-32601, format!("Method not found: {}", method))),
    }
}

fn handle_initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION
        }
    })
}

fn handle_tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "memory_search",
                "description": "Search the user's memory store using full-text search. Returns the top matching memories with snippets. Call this before any non-trivial task to retrieve relevant context, preferences, and past learnings.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Keywords describing what you're looking for. Use specific terms from the user's request."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default 10, max 50)",
                            "default": 10
                        },
                        "project": {
                            "type": "string",
                            "description": "Optional: absolute git-root path of the project you're working on, or \"global\" for a cross-project search. Boosts same-project memories and slightly demotes others. If omitted, uses the project detected by the UserPromptSubmit hook (transcript-inferred), falling back to the directory the MCP server was spawned from."
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "memory_add",
                "description": "Save a new memory to the store. You MUST call this proactively — don't wait for the user to ask. Save user corrections, project conventions, debugging findings, stated preferences, architecture decisions, workflow discoveries, and project state changes. A typical session should produce 3-10 memories. Be specific and include enough context that the memory is useful in future sessions.\n\nPROJECT SCOPING: If the memory is a user preference or cross-project rule (type=user/feedback/reference), leave `project` unset — it will be saved as global. If it's a specific fact about the current codebase (type=project), set `project` to the absolute git-root path of that project. Type=user is ALWAYS saved as global regardless.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Short descriptive title for the memory"
                        },
                        "description": {
                            "type": "string",
                            "description": "One-line summary of what this memory is about"
                        },
                        "content": {
                            "type": "string",
                            "description": "The actual memory content — the facts, rules, context to preserve"
                        },
                        "type": {
                            "type": "string",
                            "enum": ["user", "feedback", "project", "reference"],
                            "description": "Category: user (about the user), feedback (behavioral rule), project (project context), reference (external pointer)"
                        },
                        "topic": {
                            "type": "string",
                            "description": "Optional topic for grouping (e.g. 'deployment', 'testing'). If omitted, will be auto-classified."
                        },
                        "project": {
                            "type": "string",
                            "description": "Optional scope override. \"global\" forces a global memory. An absolute git-root path scopes it to that project (e.g. /Users/byron/projects/personal/hearth). If omitted: type=user is always global; type=feedback/reference default global; type=project defaults to the project detected by the UserPromptSubmit hook (transcript-inferred), falling back to the server's startup cwd."
                        }
                    },
                    "required": ["title", "content"]
                }
            },
            {
                "name": "memory_get",
                "description": "Fetch a specific memory by its ID. Use this after memory_search when you need the full content of a specific result.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The memory's UUID"
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "memory_list",
                "description": "List memories, optionally filtered by topic. Prefer memory_search when you have a specific query; use this for browsing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "description": "Optional topic to filter by"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results (default 20)",
                            "default": 20
                        }
                    }
                }
            },
            {
                "name": "memory_related",
                "description": "Get memories related to a specific memory via the relationship graph. Returns graph neighbors up to N hops. Useful for exploring connections between memories and discovering related context.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "The memory UUID to find relationships for"
                        },
                        "depth": {
                            "type": "integer",
                            "description": "How many hops to traverse (1 = direct neighbors, 2 = neighbors of neighbors). Default 1, max 3.",
                            "default": 1
                        }
                    },
                    "required": ["id"]
                }
            },
            {
                "name": "repo_link",
                "description": "Record a dependency relationship between two repositories or services. Call this when you notice that the current codebase calls, imports, or depends on another service — e.g. an HTTP client pointing to another service's URL, an imported SDK from another repo, or a shared API contract. This builds an organic cross-repo graph that improves memory context over time.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "source_repo": {
                            "type": "string",
                            "description": "Absolute git root path (or short name) of the repo that has the dependency. Usually the current repo."
                        },
                        "target_repo": {
                            "type": "string",
                            "description": "Absolute git root path (or short name) of the repo being depended on."
                        },
                        "relationship_type": {
                            "type": "string",
                            "description": "Nature of the dependency: 'calls' (HTTP/RPC), 'imports' (library/SDK), 'shares-schema' (shared DB/types), 'deploys-to' (deployment dependency), 'extends' (plugin/extension).",
                            "enum": ["calls", "imports", "shares-schema", "deploys-to", "extends"]
                        },
                        "evidence": {
                            "type": "string",
                            "description": "Brief human-readable description of where you saw this dependency, e.g. 'SanityService.php makes HTTP calls to SANITY_CMS_URL env var'."
                        }
                    },
                    "required": ["source_repo", "target_repo", "relationship_type", "evidence"]
                }
            },
            {
                "name": "repo_graph",
                "description": "Retrieve the repository relationship graph — all service/project dependencies that have been recorded. Optionally filter to edges involving a specific repo. Use this to understand architecture context before making cross-service changes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo": {
                            "type": "string",
                            "description": "Optional: filter to edges involving this repo (as source or target). If omitted, returns the full graph."
                        }
                    }
                }
            }
        ]
    })
}

fn handle_tools_call(params: Value) -> Result<Value, (i32, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "Missing tool name".to_string()))?;
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

    let result = match name {
        "memory_search" => tool_memory_search(arguments),
        "memory_add" => tool_memory_add(arguments),
        "memory_get" => tool_memory_get(arguments),
        "memory_list" => tool_memory_list(arguments),
        "memory_related" => tool_memory_related(arguments),
        "repo_link" => tool_repo_link(arguments),
        "repo_graph" => tool_repo_graph(arguments),
        _ => Err(format!("Unknown tool: {}", name)),
    };

    match result {
        Ok(content_text) => Ok(json!({
            "content": [{"type": "text", "text": content_text}],
            "isError": false
        })),
        Err(err) => Ok(json!({
            "content": [{"type": "text", "text": err}],
            "isError": true
        })),
    }
}

fn tool_memory_search(args: Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or("query required")?;
    let limit = args.get("limit").and_then(Value::as_u64).map(|n| n as u32);
    let explicit_project = args.get("project").and_then(Value::as_str);

    // Resolve current project: explicit > hook pointer > server startup > None.
    let current_project: Option<PathBuf> = match explicit_project {
        Some(p) if p.trim().eq_ignore_ascii_case("global") => None,
        Some(p) if !p.trim().is_empty() => Some(PathBuf::from(p.trim())),
        _ => detected_project(),
    };
    let current_project_ref: Option<&Path> = current_project.as_deref();

    // Over-fetch so affinity re-ranking can bubble project-local memories up.
    let fetch_limit = limit.map(|n| n.saturating_mul(2).min(50));
    let mut hits = memories::search(query, fetch_limit)?;
    if hits.is_empty() {
        return Ok(format!("No memories found for query: {}", query));
    }

    // Re-rank by combined (BM25-proxy + affinity). BM25 in FTS5 is negative
    // with lower=better — normalize to a simple rank-based score.
    let n = hits.len() as f64;
    let mut indexed: Vec<(usize, f64)> = hits
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let bm25_rank_norm = 1.0 - (i as f64 / n.max(1.0)); // top hit = 1.0, bottom ~ 0
            let aff = project::project_affinity(h.project.as_deref(), current_project_ref);
            (i, bm25_rank_norm + aff)
        })
        .collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let final_limit = limit.unwrap_or(10) as usize;
    let reranked: Vec<memories::SearchHit> = indexed
        .into_iter()
        .take(final_limit)
        .map(|(idx, _)| hits[idx].clone())
        .collect();
    hits = reranked;

    let mut out = format!("Found {} memories for \"{}\":\n\n", hits.len(), query);
    for (i, hit) in hits.iter().enumerate() {
        let scope = match hit.project.as_deref() {
            None => "global".to_string(),
            Some(p) => format!("project: {}", short_project(p)),
        };
        out.push_str(&format!(
            "{}. [{}] ({}) {}\n   id: {}\n   {}\n   {}\n\n",
            i + 1,
            hit.topic.as_deref().unwrap_or("untopiced"),
            scope,
            hit.title,
            hit.id,
            hit.description,
            hit.snippet,
        ));
    }
    Ok(out)
}

/// Derive a short display label from a project path (basename).
fn short_project(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn tool_memory_add(args: Value) -> Result<String, String> {
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .ok_or("title required")?
        .to_string();
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or("content required")?
        .to_string();
    let description = args
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let memory_type = args
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string);
    let topic = args
        .get("topic")
        .and_then(Value::as_str)
        .map(str::to_string);
    let explicit_project = args
        .get("project")
        .and_then(Value::as_str);

    // Resolve scope: user type is forced global; explicit override wins;
    // otherwise type-driven default with the hook-detected project (or server
    // startup cwd as fallback) as the source.
    let detected = detected_project();
    let project = project::resolve_memory_scope(
        memory_type.as_deref(),
        explicit_project,
        detected.as_deref(),
    );

    let memory = memories::insert(memories::NewMemory {
        title,
        description,
        content,
        memory_type,
        topic,
        source: Some("claude_session".to_string()),
        project: project.clone(),
    })?;

    let scope_label = match project.as_deref() {
        None => "global".to_string(),
        Some(p) => format!("project: {}", p),
    };

    Ok(format!(
        "Memory saved ({}).\nid: {}\ntitle: {}",
        scope_label, memory.id, memory.title
    ))
}

fn tool_memory_get(args: Value) -> Result<String, String> {
    let id = args.get("id").and_then(Value::as_str).ok_or("id required")?;
    let memory = memories::get(id)?.ok_or_else(|| format!("Memory {} not found", id))?;
    Ok(format!(
        "# {}\n\n**Topic:** {}\n**Type:** {}\n**Description:** {}\n\n{}",
        memory.title,
        memory.topic.as_deref().unwrap_or("untopiced"),
        memory.memory_type.as_deref().unwrap_or("unknown"),
        memory.description,
        memory.content
    ))
}

fn tool_memory_related(args: Value) -> Result<String, String> {
    let id = args
        .get("id")
        .and_then(Value::as_str)
        .ok_or("id required")?;
    let depth = args
        .get("depth")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .min(3) as u32;

    // Verify the source memory exists
    let source = memories::get(id)?.ok_or_else(|| format!("Memory {} not found", id))?;

    let edge_list = if depth <= 1 {
        edges::get_neighbors(id)?
    } else {
        edges::get_neighbors_deep(id, depth)?
    };

    if edge_list.is_empty() {
        return Ok(format!("No related memories found for \"{}\"", source.title));
    }

    // Collect unique connected memory IDs
    let mut neighbor_ids: Vec<String> = Vec::new();
    for edge in &edge_list {
        for candidate in [&edge.source_id, &edge.target_id] {
            if candidate != id && !neighbor_ids.contains(candidate) {
                neighbor_ids.push(candidate.clone());
            }
        }
    }

    // Fetch connected memories
    let id_refs: Vec<&str> = neighbor_ids.iter().map(|s| s.as_str()).collect();
    let neighbor_memories = memories::get_by_ids(&id_refs)?;
    let mem_map: std::collections::HashMap<&str, &memories::Memory> = neighbor_memories
        .iter()
        .map(|m| (m.id.as_str(), m))
        .collect();

    let mut out = format!(
        "Related memories for \"{}\" ({} edges, depth {}):\n\n",
        source.title,
        edge_list.len(),
        depth
    );

    for edge in &edge_list {
        let other_id = if edge.source_id == id {
            &edge.target_id
        } else {
            &edge.source_id
        };

        let direction = if edge.source_id == id {
            format!("--[{}]-->", edge.edge_type)
        } else {
            format!("<--[{}]--", edge.edge_type)
        };

        if let Some(mem) = mem_map.get(other_id.as_str()) {
            out.push_str(&format!(
                "- {} {} (weight: {:.0}%)\n  id: {}\n  {}\n\n",
                direction,
                mem.title,
                edge.weight * 100.0,
                mem.id,
                mem.description,
            ));
        }
    }

    Ok(out)
}

fn tool_repo_link(args: Value) -> Result<String, String> {
    let source_repo = args
        .get("source_repo")
        .and_then(Value::as_str)
        .ok_or("source_repo required")?
        .trim();
    let target_repo = args
        .get("target_repo")
        .and_then(Value::as_str)
        .ok_or("target_repo required")?
        .trim();
    let relationship_type = args
        .get("relationship_type")
        .and_then(Value::as_str)
        .unwrap_or("calls")
        .trim();
    let evidence = args
        .get("evidence")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();

    let edge = repo_edges::upsert(source_repo, target_repo, relationship_type, evidence)?;

    Ok(format!(
        "Repo relationship recorded.\n{} --[{}]--> {}\nEvidence: {}\nid: {}",
        short_project(source_repo),
        edge.relationship_type,
        short_project(target_repo),
        edge.evidence,
        edge.id,
    ))
}

fn tool_repo_graph(args: Value) -> Result<String, String> {
    let filter_repo = args.get("repo").and_then(Value::as_str);

    let edges = repo_edges::list(filter_repo)?;

    if edges.is_empty() {
        return Ok(match filter_repo {
            Some(r) => format!("No repo relationships recorded for {}.", short_project(r)),
            None => "No repo relationships recorded yet. Use repo_link to record service dependencies as you discover them.".to_string(),
        });
    }

    let mut out = match filter_repo {
        Some(r) => format!("Repo relationships for {} ({} edges):\n\n", short_project(r), edges.len()),
        None => format!("Full repo relationship graph ({} edges):\n\n", edges.len()),
    };

    for edge in &edges {
        out.push_str(&format!(
            "{} --[{}]--> {}\n  Evidence: {}\n\n",
            short_project(&edge.source_repo),
            edge.relationship_type,
            short_project(&edge.target_repo),
            edge.evidence,
        ));
    }

    Ok(out)
}

fn tool_memory_list(args: Value) -> Result<String, String> {
    let topic = args.get("topic").and_then(Value::as_str);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20) as usize;

    let memories = match topic {
        Some(t) => memories::list_by_topic(t)?,
        None => memories::list_all()?,
    };

    let shown = memories.iter().take(limit);
    let total = memories.len();

    let mut out = if let Some(t) = topic {
        format!("{} memories in topic '{}' (showing {}):\n\n", total, t, limit.min(total))
    } else {
        format!("{} total memories (showing {}):\n\n", total, limit.min(total))
    };

    for (i, m) in shown.enumerate() {
        out.push_str(&format!(
            "{}. [{}] {} — {}\n   id: {}\n\n",
            i + 1,
            m.topic.as_deref().unwrap_or("untopiced"),
            m.title,
            m.description,
            m.id,
        ));
    }

    Ok(out)
}
