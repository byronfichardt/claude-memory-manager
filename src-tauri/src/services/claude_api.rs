//! Subprocess wrapper for the local `claude` CLI. Used by the organizer
//! (Phase 3) to classify memories and detect duplicates.
//!
//! Uses the user's existing Claude Code authentication — no API key needed.

#![allow(dead_code)]

use serde::Serialize;
use std::path::{Path, PathBuf};
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

fn is_default_config_dir(dir: &Path) -> bool {
    dirs::home_dir().is_some_and(|h| h.join(".claude") == dir)
}

/// Give the spawned `claude` CLI the environment it needs to authenticate.
///
/// When this app is launched from Finder, launchd hands it `HOME`, `USER` and a
/// bare `PATH` — but never the exports from the user's shell profile. Without
/// `USER` the keychain lookup fails with "Not logged in · Please run /login".
///
/// `CLAUDE_CONFIG_DIR` is passed only when `config_env` is set: Claude Code
/// names its keychain entry after that variable, so a default-profile login made
/// without it is invisible to a CLI that has it, and vice versa. See
/// [`ClaudeClient::new`] for how the right choice is found.
fn apply_cli_env(cmd: &mut Command, config_env: Option<&PathBuf>) {
    let home = dirs::home_dir();

    if let Some(ref h) = home {
        cmd.env("HOME", h);
    }
    match config_env {
        Some(dir) => cmd.env("CLAUDE_CONFIG_DIR", dir),
        None => cmd.env_remove("CLAUDE_CONFIG_DIR"),
    };

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

#[derive(Debug, Clone, Default, Serialize)]
pub struct AuthStatus {
    pub logged_in: bool,
    pub config_dir: String,
    pub email: Option<String>,
    pub org_name: Option<String>,
    pub auth_method: Option<String>,
    pub error: Option<String>,
}

async fn probe_auth(binary: &str, config_env: Option<&PathBuf>) -> Result<AuthStatus, String> {
    let mut cmd = Command::new(binary);
    cmd.args(["auth", "status", "--json"]);
    apply_cli_env(&mut cmd, config_env);
    cmd.stdin(std::process::Stdio::null());

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Claude Code CLI not found at {}: {}", binary, e))?;

    // Exits non-zero when signed out, but still prints the JSON status.
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        format!("claude auth status gave no status: {}", stderr.trim())
    })?;
    let field = |key: &str| json.get(key).and_then(|v| v.as_str()).map(String::from);

    Ok(AuthStatus {
        logged_in: json.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(false),
        config_dir: field("configDirectory").unwrap_or_default(),
        email: field("email"),
        org_name: field("orgName"),
        auth_method: field("authMethod"),
        error: None,
    })
}

pub struct ClaudeClient {
    binary: String,
    model: Option<String>,
    config_dir: Option<PathBuf>,
    config_env: Option<PathBuf>,
    auth: AuthStatus,
}

pub struct AnalyzeResponse {
    pub text: String,
}

impl ClaudeClient {
    /// Resolves the profile, then whether the CLI must be told about it via
    /// `CLAUDE_CONFIG_DIR` to find its login. A non-default profile always needs
    /// it. The default `~/.claude` may have been signed into either way, so both
    /// are tried and the one that is logged in wins.
    pub async fn new(model: Option<String>) -> Self {
        let binary = resolve_claude_binary();
        let config_dir = resolve_config_dir();

        let mut candidates = vec![config_dir.clone()];
        if config_dir.as_deref().is_none_or(is_default_config_dir) {
            candidates.push(None);
        }

        let mut first: Option<(Option<PathBuf>, AuthStatus)> = None;
        for config_env in candidates {
            let auth = probe_auth(&binary, config_env.as_ref())
                .await
                .unwrap_or_else(|e| AuthStatus { error: Some(e), ..Default::default() });
            if auth.logged_in {
                first = Some((config_env, auth));
                break;
            }
            first.get_or_insert((config_env, auth));
        }
        let (config_env, mut auth) = first.unwrap_or_default();
        if auth.config_dir.is_empty() {
            auth.config_dir = config_dir
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
        }

        Self { binary, model, config_dir, config_env, auth }
    }

    pub fn auth_status(&self) -> &AuthStatus {
        &self.auth
    }

    pub fn ensure_logged_in(&self) -> Result<(), String> {
        if let Some(ref e) = self.auth.error {
            return Err(e.clone());
        }
        if !self.auth.logged_in {
            return Err(format!(
                "Not signed in to Claude Code for {}. Open Settings → Organizer account and click Sign in.",
                self.auth.config_dir
            ));
        }
        Ok(())
    }

    /// Opens Terminal running `claude auth login` for the organizer's profile, so
    /// the user never has to type a command. The OAuth flow is interactive, so it
    /// can't run headless inside the app.
    pub fn open_login(&self) -> Result<(), String> {
        let quote = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
        let env_line = match self.config_dir.as_deref() {
            Some(dir) if !is_default_config_dir(dir) => {
                format!("export CLAUDE_CONFIG_DIR={}\n", quote(&dir.to_string_lossy()))
            }
            _ => "unset CLAUDE_CONFIG_DIR\n".to_string(),
        };
        let script = format!(
            "#!/bin/zsh\n{}{} auth login\necho\necho 'Done — you can close this window and return to Claude Memory Manager.'\n",
            env_line,
            quote(&self.binary)
        );

        let path = std::env::temp_dir().join("claude-memory-manager-login.command");
        std::fs::write(&path, script).map_err(|e| format!("write login script: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| format!("make login script executable: {}", e))?;
        }

        std::process::Command::new("open")
            .arg(&path)
            .status()
            .map_err(|e| format!("open Terminal: {}", e))
            .and_then(|s| if s.success() { Ok(()) } else { Err("open Terminal failed".into()) })
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

        apply_cli_env(&mut cmd, self.config_env.as_ref());
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
}
