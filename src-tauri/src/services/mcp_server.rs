//! Minimal MCP stdio server implementation.
//!
//! Protocol: JSON-RPC 2.0 over stdin/stdout with newline-delimited framing.
//! Each message is one line of JSON terminated by `\n`.
//!
//! Implements only the minimum: initialize, tools/list, tools/call.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

use crate::store::memories;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "claude-memory-manager";
const SERVER_VERSION: &str = "0.1.1";

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
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "memory_add",
                "description": "Save a new memory to the store. Call this when you learn something worth preserving: a user correction, a project convention, a debugging finding, a stated preference. Be specific and include enough context that the memory is useful in future sessions.",
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

    let hits = memories::search(query, limit)?;
    if hits.is_empty() {
        return Ok(format!("No memories found for query: {}", query));
    }

    let mut out = format!("Found {} memories for \"{}\":\n\n", hits.len(), query);
    for (i, hit) in hits.iter().enumerate() {
        out.push_str(&format!(
            "{}. [{}] {}\n   id: {}\n   {}\n   {}\n\n",
            i + 1,
            hit.topic.as_deref().unwrap_or("untopiced"),
            hit.title,
            hit.id,
            hit.description,
            hit.snippet,
        ));
    }
    Ok(out)
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

    let memory = memories::insert(memories::NewMemory {
        title,
        description,
        content,
        memory_type,
        topic,
        source: Some("claude_session".to_string()),
    })?;

    Ok(format!(
        "Memory saved.\nid: {}\ntitle: {}",
        memory.id, memory.title
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
