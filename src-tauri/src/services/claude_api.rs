//! Subprocess wrapper for the local `claude` CLI. Used by the organizer
//! (Phase 3) to classify memories and detect duplicates.
//!
//! Uses the user's existing Claude Code authentication — no API key needed.

#![allow(dead_code)]

use tokio::process::Command;

fn resolve_claude_binary() -> String {
    crate::services::bootstrap::claude_binary_path()
}

pub struct ClaudeClient {
    binary: String,
    model: Option<String>,
}

pub struct AnalyzeResponse {
    pub text: String,
}

impl Default for ClaudeClient {
    fn default() -> Self {
        Self::new(None)
    }
}

impl ClaudeClient {
    pub fn new(model: Option<String>) -> Self {
        Self {
            binary: resolve_claude_binary(),
            model,
        }
    }

    /// Send a prompt to Claude via the CLI and return the text response.
    ///
    /// This is called from the organizer which runs analysis work. We take
    /// care to spawn `claude -p` in a minimal mode that skips:
    /// - MCP server loading (critical: otherwise our own MCP server recurses)
    /// - Tool registration
    /// - Slash commands / skills
    ///
    /// This dramatically reduces startup cost and eliminates recursive subprocess
    /// spawns (which also eliminates WithSecure XFENCE prompts during organize).
    pub async fn analyze(
        &self,
        system: &str,
        prompt: &str,
    ) -> Result<AnalyzeResponse, String> {
        let full_prompt = format!("{}\n\n---\n\n{}", system, prompt);

        let mut cmd = Command::new(&self.binary);
        cmd.arg("-p")
            .arg(&full_prompt)
            .arg("--output-format")
            .arg("text")
            // Skip loading any MCP servers (critical — prevents recursive spawning
            // of our own MCP server from within the organizer).
            .arg("--strict-mcp-config")
            .arg("--mcp-config")
            .arg(r#"{"mcpServers":{}}"#)
            // Disable all tools — we only want text generation.
            .arg("--tools")
            .arg("")
            // Disable skills / slash commands.
            .arg("--disable-slash-commands");

        if let Some(ref model) = self.model {
            cmd.arg("--model").arg(model);
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Failed to spawn 'claude' CLI: {}. Is Claude Code installed?", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("claude CLI exited with error: {}", stderr.trim()));
        }

        let text = String::from_utf8(output.stdout)
            .map_err(|e| format!("claude CLI returned invalid UTF-8: {}", e))?;

        Ok(AnalyzeResponse {
            text: text.trim().to_string(),
        })
    }

    pub async fn check_available(&self) -> Result<(), String> {
        let output = Command::new(&self.binary)
            .arg("--version")
            .output()
            .await
            .map_err(|e| format!("Claude Code CLI not found: {}", e))?;

        if !output.status.success() {
            return Err("claude --version failed".to_string());
        }
        Ok(())
    }
}
