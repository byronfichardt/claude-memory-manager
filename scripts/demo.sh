#!/usr/bin/env bash
#
# Launch Claude Memory Manager in a sandboxed demo environment with fake
# memories and an isolated HOME — used for taking screenshots without
# exposing any personal data.
#
# Usage:
#   ./scripts/demo.sh          Launch the demo app
#   ./scripts/demo.sh --clean  Remove the demo home dir (cleanup)
#
# The demo HOME lives at /tmp/cmm-demo/ by default. Override via CMM_DEMO_HOME.

set -euo pipefail

DEMO_HOME="${CMM_DEMO_HOME:-/tmp/cmm-demo}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$REPO_ROOT/src-tauri/target/release/claude-memory-manager"

if [[ "${1:-}" == "--clean" ]]; then
  echo "Removing $DEMO_HOME"
  rm -rf "$DEMO_HOME"
  echo "Done."
  exit 0
fi

# Build the release binary if needed
if [[ ! -f "$BIN" ]]; then
  echo "Release binary not found. Building..."
  (cd "$REPO_ROOT/src-tauri" && cargo build --release)
fi

# Wipe any previous demo state
echo "Setting up fake HOME at $DEMO_HOME"
rm -rf "$DEMO_HOME"
mkdir -p "$DEMO_HOME/.claude/projects"
mkdir -p "$DEMO_HOME/.claude-memory-manager"

# Seed the demo database
echo "Seeding demo database..."
python3 "$REPO_ROOT/scripts/demo-seed.py" "$DEMO_HOME/.claude-memory-manager/memories.db"

# Create a stub CLAUDE.md with the managed-section markers so the app considers
# itself "set up" and goes straight to the Home topics view (skipping the
# first-run welcome card).
cat > "$DEMO_HOME/.claude/CLAUDE.md" <<'EOF'
<!-- claude-memory-manager:start -->
## Memory

Demo stub — this is a fake CLAUDE.md used for screenshots. Not a real config.
<!-- claude-memory-manager:end -->
EOF
echo "Created stub ~/.claude/CLAUDE.md with managed section markers"

# Create a stub .claude.json with a fake MCP server registration so the app
# hides the "Connect to Claude Code" banner. The binary path is fictional —
# nothing will actually spawn from it since we only take screenshots.
cat > "$DEMO_HOME/.claude/.claude.json" <<EOF
{
  "mcpServers": {
    "claude-memory-manager": {
      "type": "stdio",
      "command": "$BIN",
      "args": ["--mcp-server"],
      "env": {}
    }
  }
}
EOF
echo "Created stub ~/.claude/.claude.json with MCP registration"

# Also create a stub settings.json with the permission grant + hook entry
# so the Settings view shows everything as "installed" and "registered".
cat > "$DEMO_HOME/.claude/settings.json" <<EOF
{
  "permissions": {
    "allow": [
      "mcp__claude-memory-manager"
    ]
  },
  "hooks": {
    "UserPromptSubmit": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "$BIN --hook",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
EOF
echo "Created stub ~/.claude/settings.json with hook + permissions"

# Launch with the fake HOME
echo ""
echo "============================================"
echo "Launching demo app"
echo "  HOME     = $DEMO_HOME"
echo "  Binary   = $BIN"
echo ""
echo "Everything the app writes goes to $DEMO_HOME."
echo "Your real memory store at ~/.claude-memory-manager/ is untouched."
echo ""
echo "When you're done, run:  ./scripts/demo.sh --clean"
echo "============================================"
echo ""

HOME="$DEMO_HOME" "$BIN"
