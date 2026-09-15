//! Subprocess wrapper for the local `claude` CLI. Used by the organizer
//! (Phase 3) to classify memories and detect duplicates.
//!
//! Uses the user's existing Claude Code authentication — no API key needed.

#![allow(dead_code)]

use std::path::PathBuf;
use tokio::process::Command;

use crate::services::bootstrap::list_claude_config_dirs;
use crate::store::settings;

fn resolve_claude_binary() -> String {
    crate::services::bootstrap::claude_binary_path()
}

/// Absolute path of the `~/.claude*` dir the organizer authenticates as.
/// Empty or unset means "resolve automatically".
pub const SETTING_ORGANIZER_CONFIG_DIR: &str = "organizer_config_dir";

/// Config directory the spawned CLI authenticates against.
///
/// The store ingests memories from every detected profile, but each `claude -p`
/// call bills exactly one account. Precedence: the user's explicit choice in
/// Settings, then the `CLAUDE_CONFIG_DIR` this process was launched with, then
/// the stock `~/.claude`, then whichever profile was detected first.
pub fn resolve_config_dir() -> Option<PathBuf> {
    let configured = settings::get(SETTING_ORGANIZER_CONFIG_DIR, "").unwrap_or_default();
    if !configured.is_empty() {
        let path = PathBuf::from(&configured);
        if path.is_dir() {
            return Some(path);
        }
    }

    if let Ok(env_dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        let path = PathBuf::from(&env_dir);
        if !env_dir.is_empty() && path.is_dir() {
            return Some(path);
        }
    }

    let detected = list_claude_config_dirs();
    detected
        .iter()
        .find(|(label, _)| label == "default")
        .or_else(|| detected.first())
        .map(|(_, path)| path.clone())
}

/// Give the spawned `claude` CLI the environment it needs to authenticate.
///
/// When this app is launched from Finder, launchd hands it `HOME`, `USER` and a
/// bare `PATH` — but never the exports from the user's shell profile. Claude
/// Code's credential lookup needs more than that, and each missing piece fails
/// differently:
///
///   * no `USER`              -> "Not logged in · Please run /login"
///   * no `CLAUDE_CONFIG_DIR` -> "OAuth session expired and could not be refreshed"
///
/// Both must be set or every organizer phase that shells out to the CLI fails,
/// while the pure-SQL phases (edge prune) quietly succeed — which reads as
/// "organize ran but classified nothing".
///
/// This previously worked only by accident: the app happened to be launched from
/// a terminal, so it inherited these from the shell. Relaunching it from Finder
/// broke classification with no visible error.
fn apply_cli_env(cmd: &mut Command, config_dir: Option<&PathBuf>) {
    let home = dirs::home_dir();

    if let Some(ref h) = home {
        cmd.env("HOME", h);
    }
    if let Some(dir) = config_dir {
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }

    // Keychain lookup fails outright without a username. Fall back to the home
    // directory's name, which matches the account name on macOS.
    let user = std::env::var("USER")
        .ok()
        .or_else(|| std::env::var("LOGNAME").ok())
        .or_else(|| {
            home.as_ref()
                .and_then(|h| h.file_name())
                .map(|n| n.to_string_lossy().to_string())
        });
    if let Some(user) = user {
        cmd.env("USER", &user);
        cmd.env("LOGNAME", &user);
    }

    // A Finder-launched app's PATH omits the usual install locations; the CLI
    // shells out to `security` (keychain) and `git`, so keep them reachable.
    let mut path = std::env::var("PATH").unwrap_or_default();
    for extra in ["/usr/bin", "/bin", "/usr/sbin", "/sbin", "/usr/local/bin"] {
        if !path.split(':').any(|p| p == extra) {
            if !path.is_empty() {
                path.push(':');
            }
            path.push_str(extra);
        }
    }
    if let Some(ref h) = home {
        let local_bin = h.join(".local/bin");
        let local_bin = local_bin.to_string_lossy();
        if !path.split(':').any(|p| p == local_bin) {
            path.push(':');
            path.push_str(&local_bin);
        }
    }
    cmd.env("PATH", path);
}

pub struct ClaudeClient {
    binary: String,
    model: Option<String>,
    config_dir: Option<PathBuf>,
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
            config_dir: resolve_config_dir(),
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

        apply_cli_env(&mut cmd, self.config_dir.as_ref());
        // The CLI waits ~3s for stdin before giving up; close it so each call
        // doesn't pay that penalty.
        cmd.stdin(std::process::Stdio::null());

        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Failed to spawn 'claude' CLI: {}. Is Claude Code installed?", e))?;

        if !output.status.success() {
            // Auth failures ("Not logged in", "OAuth session expired") are
            // written to stdout, not stderr — reporting stderr alone yields an
            // empty, undiagnosable error.
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = match (stderr.trim(), stdout.trim()) {
                ("", "") => "no output".to_string(),
                ("", out) => out.to_string(),
                (err, "") => err.to_string(),
                (err, out) => format!("{} / {}", err, out),
            };
            return Err(format!("claude CLI exited with error: {}", detail));
        }

        let text = String::from_utf8(output.stdout)
            .map_err(|e| format!("claude CLI returned invalid UTF-8: {}", e))?;

        Ok(AnalyzeResponse {
            text: text.trim().to_string(),
        })
    }

    pub async fn check_available(&self) -> Result<(), String> {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("--version");
        apply_cli_env(&mut cmd, self.config_dir.as_ref());
        cmd.stdin(std::process::Stdio::null());

        let output = cmd
            .output()
            .await
            .map_err(|e| format!("Claude Code CLI not found: {}", e))?;

        if !output.status.success() {
            return Err("claude --version failed".to_string());
        }
        Ok(())
    }
}
