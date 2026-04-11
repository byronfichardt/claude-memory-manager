//! Claude Code UserPromptSubmit hook handler.
//!
//! Invoked by Claude Code on every user message (before Claude processes it).
//! Reads a JSON event from stdin, queries the memory store with the prompt text,
//! and writes relevant memories to stdout as additional context.
//!
//! This gives deterministic memory retrieval — Claude sees relevant memories in
//! every turn without having to choose to call the memory_search tool.
//!
//! Exit 0 = allow the prompt to proceed (always, even on error — we never block).
//! Stdout text is injected into Claude's context for this turn.

use std::io::{self, Read, Write};

use crate::store::{self, memories};

const MAX_RESULTS: u32 = 5;
const MAX_SNIPPET_CHARS: usize = 300;
const MIN_PROMPT_LEN: usize = 4;
const MAX_TITLE_LEN: usize = 80;

#[derive(serde::Deserialize)]
struct HookEvent {
    #[serde(default)]
    prompt: String,
    #[allow(dead_code)]
    #[serde(default)]
    session_id: String,
    #[allow(dead_code)]
    #[serde(default)]
    cwd: String,
    #[allow(dead_code)]
    #[serde(default)]
    hook_event_name: String,
}

pub fn run() -> Result<(), String> {
    // Read JSON event from stdin
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("read stdin: {}", e))?;

    if input.trim().is_empty() {
        return Ok(());
    }

    let event: HookEvent = serde_json::from_str(&input)
        .map_err(|e| format!("parse hook event: {}", e))?;

    // Don't bother with very short prompts ("ok", "yes", etc.)
    if event.prompt.trim().len() < MIN_PROMPT_LEN {
        return Ok(());
    }

    // Initialize the store (cheap with WAL mode SQLite)
    store::init().map_err(|e| format!("store init: {}", e))?;

    let mut out = String::new();

    // Check for explicit "remember this" directive. If found, save it directly
    // as a deterministic manual memory.
    if let Some(text_to_save) = extract_remember_directive(&event.prompt) {
        match save_user_memory(text_to_save) {
            Ok(title) => {
                out.push_str("<memory-saved>\n");
                out.push_str(&format!(
                    "✓ Memory saved automatically via the user's `remember:` directive.\nTitle: {}\n",
                    title
                ));
                out.push_str("You do NOT need to call memory_add for this — it's already saved.\n");
                out.push_str("Acknowledge the save briefly and proceed with any other part of the user's request.\n");
                out.push_str("</memory-saved>\n\n");
            }
            Err(e) => {
                eprintln!("[hook] remember save failed: {}", e);
                // Fall through to normal retrieval anyway
            }
        }
    }

    // Search the memory store using the prompt as a query
    let hits = memories::search(&event.prompt, Some(MAX_RESULTS))
        .map_err(|e| format!("search: {}", e))?;

    if !hits.is_empty() {
        out.push_str("<memory-context>\n");
        out.push_str("Relevant memories from your persistent memory store (retrieved automatically):\n\n");

        for (i, hit) in hits.iter().enumerate() {
            let topic = hit.topic.as_deref().unwrap_or("untopiced");
            out.push_str(&format!("{}. **{}** _{}_", i + 1, hit.title, topic));
            if !hit.description.is_empty() {
                out.push_str(&format!(" — {}", hit.description));
            }
            out.push('\n');

            let snippet = clean_snippet(&hit.snippet);
            if !snippet.is_empty() {
                let truncated = truncate_chars(&snippet, MAX_SNIPPET_CHARS);
                out.push_str(&format!("   {}\n", truncated));
            }
            out.push('\n');
        }

        out.push_str("</memory-context>\n");
    }

    if out.is_empty() {
        // Nothing to inject — Claude proceeds as normal
        return Ok(());
    }

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    handle
        .write_all(out.as_bytes())
        .map_err(|e| format!("write stdout: {}", e))?;
    handle.flush().map_err(|e| format!("flush: {}", e))?;

    Ok(())
}

/// Detects explicit save-to-memory directives at the start of a user prompt.
/// Returns the text content to save if found.
///
/// Recognized patterns (case-insensitive on the prefix):
/// - "remember: <text>"
/// - "remember that <text>"
/// - "/remember <text>"
/// - "!remember <text>"
fn extract_remember_directive(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    let lower = trimmed.to_lowercase();

    const PREFIXES: &[&str] = &[
        "remember: ",
        "remember:",
        "remember that ",
        "/remember ",
        "/remember:",
        "!remember ",
        "!remember:",
    ];

    for prefix in PREFIXES {
        if lower.starts_with(prefix) {
            let rest = trimmed[prefix.len()..].trim();
            if rest.len() >= MIN_PROMPT_LEN {
                return Some(rest.to_string());
            }
        }
    }

    None
}

/// Save a user-directed memory. Derives a title from the first sentence or
/// first MAX_TITLE_LEN chars. Content is the full text. Returns the title.
fn save_user_memory(text: String) -> Result<String, String> {
    let title = derive_title(&text);
    let description = if title.len() < text.len() {
        String::new()
    } else {
        String::new()
    };

    let memory = memories::insert(memories::NewMemory {
        title: title.clone(),
        description,
        content: text,
        memory_type: Some("user".to_string()),
        topic: None,
        source: Some("user_remember_directive".to_string()),
    })?;

    Ok(memory.title)
}

fn derive_title(text: &str) -> String {
    let trimmed = text.trim();
    // Take first sentence or first N chars, whichever is shorter
    let first_sentence = trimmed
        .split(|c| c == '.' || c == '!' || c == '?' || c == '\n')
        .next()
        .unwrap_or(trimmed)
        .trim();

    if first_sentence.chars().count() <= MAX_TITLE_LEN {
        first_sentence.to_string()
    } else {
        let truncated: String = first_sentence.chars().take(MAX_TITLE_LEN).collect();
        format!("{}...", truncated)
    }
}

/// Clean FTS5 snippet markers. SearchHit's snippet has `[word]` around matches.
fn clean_snippet(s: &str) -> String {
    s.replace('[', "").replace(']', "").trim().to_string()
}

fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(n).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_remember_directive() {
        assert_eq!(
            extract_remember_directive("remember: we use port 5432 for postgres"),
            Some("we use port 5432 for postgres".to_string())
        );
        assert_eq!(
            extract_remember_directive("Remember: use kebab-case for new files"),
            Some("use kebab-case for new files".to_string())
        );
        assert_eq!(
            extract_remember_directive("/remember the backup runs at 3am"),
            Some("the backup runs at 3am".to_string())
        );
        assert_eq!(
            extract_remember_directive("!remember skip tests locally"),
            Some("skip tests locally".to_string())
        );
        assert_eq!(
            extract_remember_directive("remember that apples are red"),
            Some("apples are red".to_string())
        );
        // Too short
        assert_eq!(extract_remember_directive("remember: a"), None);
        // No directive
        assert_eq!(extract_remember_directive("how do I deploy this"), None);
        // Partial match — "remember" without colon shouldn't trigger
        assert_eq!(extract_remember_directive("I need to remember to update"), None);
    }

    #[test]
    fn test_derive_title() {
        assert_eq!(derive_title("short fact"), "short fact");
        assert_eq!(derive_title("First sentence. More details here"), "First sentence");
        assert_eq!(
            derive_title("Port 5432 is used for postgres on the production server"),
            "Port 5432 is used for postgres on the production server"
        );
        // Long first sentence gets truncated with ellipsis
        let long = "x".repeat(100);
        let title = derive_title(&long);
        assert!(title.ends_with("..."));
        assert!(title.len() <= MAX_TITLE_LEN + 3);
    }
}
