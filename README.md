# Claude Memory Manager

An automated, self-organizing memory system for [Claude Code](https://code.claude.com). Install once and Claude Code gains persistent, cross-session memory that works from any directory — no copy-paste, no manual tool calls, no reliance on Claude remembering to save.

**The goal**: after setup, Claude should _always_ have relevant context from your past conversations, in every new session, without you thinking about it.

---

## What it does

1. **Ingests** your existing Claude Code memory files (`~/.claude*/projects/*/memory/`) into a central SQLite store.
2. **Injects relevant memories automatically** into every Claude Code session via a `UserPromptSubmit` hook — Claude sees matching memories before it processes each message.
3. **Saves new memories** when Claude decides to (via an MCP `memory_add` tool) or when you use a `remember:` directive in your prompt.
4. **Organizes automatically** — runs an AI-powered classification + deduplication pass on launch to keep the store clean.
5. **Provides a tiny UI** for browsing, searching, and editing memories when you want to — the app is meant to be mostly invisible.

## How it works

```
┌─────────────────────────────────────┐         ┌──────────────────────────┐
│  Your Claude Code session           │         │  Claude Memory Manager   │
│  (any directory, any project)       │         │  (Tauri desktop app)     │
└────────────────┬────────────────────┘         └────────────┬─────────────┘
                 │                                            │
                 │  1. You type a message                     │
                 │  ────────────────────────────────────▶     │
                 │                                            │
                 │  2. UserPromptSubmit hook runs             │
                 │     (claude-memory-manager --hook)         │
                 │  ◀────────────────────────────────────     │
                 │                                            ▼
                 │                               ┌──────────────────────┐
                 │                               │   SQLite + FTS5      │
                 │                               │   Full-text search   │
                 │                               │   ~5ms per query     │
                 │                               └──────────┬───────────┘
                 │                                           │
                 │  3. Top memories injected as context     │
                 │  ◀──────────────────────────────────     │
                 │                                           │
                 │  4. Claude replies — optionally           │
                 │     calling `memory_add` via MCP          │
                 │  ─────────────────────────────────▶      │
```

Two independent pathways feed into the store:

- **UserPromptSubmit hook** (reads) — fires on every user message, queries the store using FTS5, injects up to 5 relevant memories as a `<memory-context>` block.
- **MCP server** (reads & writes) — exposes `memory_search`, `memory_add`, `memory_get`, and `memory_list` tools for Claude to call when it needs targeted lookups or wants to save something new. Registered at user scope (`claude mcp add --scope user`).

Both are installed automatically when you click "Get started" the first time.

## Key features

### Automatic memory retrieval (the reason this exists)

Prior to this tool, Claude Code memories only help if Claude happens to call `memory_search` in a session. Most sessions don't. The `UserPromptSubmit` hook fixes that: every message Claude sees, it already has the relevant memories in context. No tool call overhead, no Claude forgetting.

### AI-powered organization

On every launch (or manually), the organizer runs a 3-phase pass:
- **Classify** untopiced memories into topics via `claude -p` (batched, 25 at a time)
- **Dedup** within each topic — merges near-duplicates with full content reconciliation
- **Consolidate** overlapping topics — merges single-member or semantically-redundant topics into broader ones

Every destructive operation is snapshotted to a history log, so you can undo.

### `remember:` directive
Type `remember: <anything>` (or `/remember ...`, `!remember ...`) in any Claude Code session. The hook intercepts it, saves the text to the store as a new memory, and tells Claude not to bother calling `memory_add`. Instant deterministic save.

### Multi-account support

Detects all `~/.claude*` directories automatically (`.claude`, `.claude-personal`, `.claude-work`, etc.), installs the hook + MCP server + bootstrap prompt in each one. Works out of the box for people running multiple Claude Code profiles via `CLAUDE_CONFIG_DIR`.

### Tiny UI

- **Home**: topics grid with counts, one-click organize, status ribbon
- **Topic detail**: list of memories, preview/edit mode (markdown rendering), inline delete
- **Search**: FTS5 search across everything with highlighted snippets
- **Settings**: toggles for MCP registration, hook, auto-organize + per-config status table

Most users should rarely need to open the app — it's the settings/maintenance panel for an otherwise invisible system.

## Performance

Measured on an Apple M-series Mac, 42 memories in the store, release build:

| Metric                             | Value          |
|------------------------------------|----------------|
| Hook invocation (per user message) | **48–54 ms**   |
| FTS5 search over 42 memories       | ~5 ms          |
| Token cost per turn                | 140–220 tokens |
| Dollar cost per turn (Sonnet)      | ~$0.0006       |
| MCP server cold start              | <100 ms        |
| Settings page load                 | ~40 ms         |

Latency stays flat as the store grows — FTS5 + WAL mode handles 10K+ memories comfortably. Short prompts (< 4 chars) short-circuit without running a query.

**Versus the alternative** (Claude calling `memory_search` as a tool): the hook is roughly 60× faster per turn and always runs, whereas tool calls require Claude to decide it needs context (which it often doesn't).

## Architecture

- **Rust (Tauri 2)** backend
  - `src-tauri/src/store/` — SQLite with FTS5 (porter tokenizer), memories/topics/history/settings tables, WAL mode
  - `src-tauri/src/services/hook.rs` — UserPromptSubmit hook handler (stdin JSON in, markdown context out)
  - `src-tauri/src/services/mcp_server.rs` — Minimal stdio MCP server (JSON-RPC, no heavy SDK)
  - `src-tauri/src/services/organizer.rs` — Classify / dedup / consolidate phases
  - `src-tauri/src/services/bootstrap.rs` — Manages `~/.claude*/CLAUDE.md` + `settings.json`
  - `src-tauri/src/services/ingestion.rs` — One-shot import of existing memory files
  - `src-tauri/src/services/claude_api.rs` — `claude -p` subprocess wrapper (with `--strict-mcp-config` to prevent MCP recursion)
- **Vue 3** frontend
  - Single Pinia store (`stores/app.ts`)
  - 4 views: Home, Topic, Search, Settings
  - Markdown rendering via [`marked`](https://marked.js.org)

The same binary has three modes, selected by command-line flag:

| Mode           | How invoked                       | What it does                                           |
|----------------|-----------------------------------|--------------------------------------------------------|
| UI (default)   | `claude-memory-manager`           | Opens the Tauri window                                 |
| MCP server     | `claude-memory-manager --mcp-server` | stdio JSON-RPC MCP protocol                          |
| Hook           | `claude-memory-manager --hook`    | Reads JSON from stdin, writes context to stdout       |

## Install

### Prerequisites

- **macOS** (arm64 prebuilt binary currently available — see Releases; other platforms need a source build)
- **Claude Code CLI** installed and authenticated (`claude auth` must work)
- **Rust toolchain** + **Node.js 20+** (only needed for building from source)

### From source

```bash
git clone https://github.com/byronfichardt/claude-memory-manager
cd claude-memory-manager

# Install frontend deps
npm install

# Build the release binary (takes ~1 minute)
cd src-tauri
cargo build --release
cd ..

# Run the UI
./src-tauri/target/release/claude-memory-manager
```

On first launch, click **Get started**. The app will:
1. Scan `~/.claude*/projects/*/memory/` for existing memory files
2. Ingest them into `~/.claude-memory-manager/memories.db`
3. Write a managed section to each `~/.claude*/CLAUDE.md`
4. Register the MCP server in each Claude config (`claude mcp add --scope user`)
5. Install the UserPromptSubmit hook in each Claude config's `settings.json`
6. Auto-run an organize pass (if there are memories to classify)

After setup, open any Claude Code session — the hook will start injecting relevant memories automatically.

### Dev mode

```bash
npm run tauri:dev
```

Spawns Vite + Tauri in watch mode. Frontend hot-reloads automatically; Rust changes require closing the app window so the dev process re-spawns the binary.

## Build targets

Currently prebuilt:

| Platform            | Binary                                                           |
|---------------------|------------------------------------------------------------------|
| **macOS 15+ arm64** (Apple Silicon) | See [Releases](https://github.com/byronfichardt/claude-memory-manager/releases) |

Other platforms are source-buildable via `cargo build --release`, but untested.

## Configuration

Everything is stored in `~/.claude-memory-manager/`:

```
~/.claude-memory-manager/
├── memories.db         # SQLite database
├── memories.db-wal     # write-ahead log
└── memories.db-shm     # shared memory
```

If your endpoint protection tool watches dot-folders in `$HOME` (e.g. WithSecure XFENCE), you can override the DB location via the `CLAUDE_MEMORY_DB_DIR` environment variable — set it in Settings → Memory store, and the MCP server will be re-registered with the env var.

Per-Claude-config files the app writes to:

```
~/.claude/
├── CLAUDE.md           # managed section between <!-- claude-memory-manager:start --> markers
├── settings.json       # hook entry + MCP permission grant
└── .claude.json        # MCP server registration (managed by `claude mcp add`)
```

Everything outside those markers/keys is left untouched.

## Uninstall

The app has no automatic uninstaller yet. To fully remove:

```bash
# 1. Unregister MCP server (one per Claude config dir)
CLAUDE_CONFIG_DIR=~/.claude claude mcp remove claude-memory-manager --scope user
CLAUDE_CONFIG_DIR=~/.claude-personal claude mcp remove claude-memory-manager --scope user  # if present

# 2. Remove the managed section from each ~/.claude*/CLAUDE.md
#    (edit by hand — delete the block between <!-- claude-memory-manager:start --> and :end -->)

# 3. Remove the hook entry from each ~/.claude*/settings.json
#    (edit by hand — remove the entry from hooks.UserPromptSubmit that references claude-memory-manager)

# 4. Remove the mcp__claude-memory-manager permission from each ~/.claude*/settings.json

# 5. Delete the database
rm -rf ~/.claude-memory-manager
```

## Troubleshooting

### "I don't see memories being injected"

- Check Settings → MCP Server and Settings → Auto memory injection. Both should show as registered/installed.
- Verify the hook works: `echo '{"prompt":"docker","session_id":"x","cwd":"/"}' | /path/to/claude-memory-manager --hook`
- If you recently moved the binary, re-click "Register / Re-register" in Settings to update the paths.

### "WithSecure XFENCE keeps asking for permission"

The unsigned debug binary is the culprit. Options:
1. In WithSecure → Application Control / Folder Shield, add the binary path as trusted.
2. Build a release binary (`cargo build --release`) — roughly 2× faster startup and sometimes recognized.
3. Move the DB out of `~/.claude-memory-manager` by setting `CLAUDE_MEMORY_DB_DIR` in Settings.

### "Organize picked bad topics / merged things I didn't want"

- Settings → Organization → **Undo last** reverts the most recent destructive operation.
- Run "Organize now" again after editing memories manually.
- The consolidate prompt is tuned to be conservative but occasionally picks questionable merges.

### "The app slowed down during organize"

Fixed in current builds via `--strict-mcp-config`, which prevents `claude -p` from recursively spawning our own MCP server during the organize pass. If you see this on an older build, pull latest and `cargo build --release`.

## License

[MIT](./LICENSE)

## Credits

- [Tauri 2](https://tauri.app) — desktop shell
- [Vue 3](https://vuejs.org) + [Pinia](https://pinia.vuejs.org) — frontend
- [rusqlite](https://github.com/rusqlite/rusqlite) + SQLite FTS5 — storage
- [marked](https://marked.js.org) — markdown rendering
- [Model Context Protocol](https://modelcontextprotocol.io) — for MCP server integration
- [Claude Code](https://code.claude.com) — the thing this is for
