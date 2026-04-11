# Claude Memory Manager v0.1.0

First public release. Automated, self-organizing memory system for Claude Code.

## Downloads

| Asset | Platform | Size |
|---|---|---|
| `claude-memory-manager-0.1.0-macos-arm64.dmg` | **macOS 11+ Apple Silicon (arm64)** | 4.7 MB |
| `claude-memory-manager-0.1.0-macos-arm64.zip` | **macOS 11+ Apple Silicon (arm64)** — alternative if you prefer an unpacked `.app` | 4.2 MB |
| `checksums.sha256` | SHA-256 checksums for both artifacts | — |

> ⚠️ These prebuilt binaries are **macOS arm64 only** — they will not run on Intel Macs, Linux, or Windows. For other platforms, build from source (see below).

## What's in the box

- `Claude Memory Manager.app` — the Tauri desktop app
- All three binary modes bundled into the single executable:
  - Default: Tauri UI window
  - `--mcp-server`: stdio MCP server mode (invoked by Claude Code)
  - `--hook`: UserPromptSubmit hook handler (invoked by Claude Code)

## Platform requirements

- **macOS 11 Big Sur or later** (Apple Silicon M1/M2/M3/M4)
- **Claude Code CLI** already installed and authenticated — `claude --version` must work
- A `~/.claude` directory (created automatically on Claude Code first run)

**Not supported** in this prebuilt:
- Intel Macs (x86_64) — build from source
- Linux (any arch) — build from source
- Windows — untested, build from source

## Install

**Option A — DMG (recommended)**:

1. Download `claude-memory-manager-0.1.0-macos-arm64.dmg`
2. Double-click to mount
3. Drag `Claude Memory Manager.app` to the `Applications` shortcut in the mounted volume
4. Eject the DMG, then launch the app from Applications

**Option B — Zip**:

1. Download `claude-memory-manager-0.1.0-macos-arm64.zip`
2. Unzip to get `Claude Memory Manager.app`
3. Move it to `/Applications` (recommended)

**First launch — important**:

The binary is **not code-signed**, and macOS 15+ no longer accepts the old "right-click → Open" bypass for quarantined unsigned apps. Instead, you'll see a misleading **"Claude Memory Manager is damaged and can't be opened"** error on double-click. **The file is not actually damaged** — this is just macOS's quarantine flag.

**Click Cancel** on that dialog, then run this in Terminal to strip the quarantine attribute:

```bash
xattr -dr com.apple.quarantine "/Applications/Claude Memory Manager.app"
```

(Adjust the path if the `.app` is somewhere other than `/Applications`.)

After that one command, future launches work normally.

To verify the file wasn't tampered with in transit, compare its SHA-256 against `checksums.sha256` from this release:

```bash
shasum -a 256 ~/Downloads/claude-memory-manager-0.1.0-macos-arm64.dmg
```

Once the app opens, click **Get started**. It will:
   - Ingest existing memory files from every `~/.claude*/projects/*/memory/` it finds
   - Write a managed bootstrap section to each `~/.claude*/CLAUDE.md`
   - Register the MCP server at user scope (`claude mcp add --scope user`)
   - Install the UserPromptSubmit hook in each `~/.claude*/settings.json`
   - Run an initial organize pass (AI-powered topic classification + deduplication)

## Verify it works

After setup, open a new Claude Code session in any directory and ask something that should match a memory you've seen before:

```bash
claude -p "what do you know about my deployment setup?"
```

If the hook is wired correctly, Claude will answer using facts from your memory store — no tool calls needed. You should also see the memory store populated in the app's Home view under topic categories.

## What this release includes

- **Automatic retrieval via UserPromptSubmit hook** — relevant memories injected into every Claude Code prompt (~50 ms, ~200 tokens per message)
- **MCP server** exposing `memory_search`, `memory_add`, `memory_get`, `memory_list` tools
- **`remember:` directive** — type `remember: <anything>` in any Claude session to save instantly
- **Auto-organization** — AI classifies untopiced memories, dedups near-duplicates, consolidates overlapping topics
- **Multi-config support** — detects any `~/.claude*` directory with `projects/` or `.claude.json`
- **Simple UI** — Home (topic grid), Topic detail (preview/edit with markdown), Search (FTS5), Settings
- **Undo log** for destructive organize operations

## Known issues

- **Unsigned binary** — the app is not code-signed with an Apple Developer certificate. On first download, macOS puts a quarantine attribute on the file, and opening it shows a "damaged and can't be opened" error. This is **not** actual damage — it's just Apple's Gatekeeper being strict about unsigned apps. Strip the attribute with `xattr -dr com.apple.quarantine "/Applications/Claude Memory Manager.app"` (see the Install section above for details). Endpoint protection tools may also prompt on first launch; if yours does, add the `Claude Memory Manager.app` path to its trusted applications list (one-time setup). The long-term fix is code signing + notarization with an Apple Developer cert — that's not in this release.
- **Plain DMG layout** — the DMG is a functional drag-to-Applications installer but doesn't have a custom background image or pre-positioned icons. You'll see the app and an Applications shortcut in the mounted volume; drag the app onto the shortcut. (The standard Tauri DMG prettification requires macOS Automation permission, which isn't reliably available in automated builds.)
- **The managed section** in your existing `~/.claude*/CLAUDE.md` files is preserved between the `<!-- claude-memory-manager:start -->` and `<!-- claude-memory-manager:end -->` markers. Don't edit inside those markers — they'll be overwritten on re-register. Everything outside is left alone.

## Build from source (any platform)

If you're not on macOS arm64 or want to build yourself:

```bash
git clone https://github.com/byronfichardt/claude-memory-manager
cd claude-memory-manager
npm install
cd src-tauri
cargo build --release
```

The binary will be at `src-tauri/target/release/claude-memory-manager`. For a full `.app` bundle on macOS, run `npm run tauri:build` from the project root instead.

Requires:
- Rust stable (1.80+)
- Node.js 20+
- Tauri build prerequisites for your OS (see [Tauri docs](https://v2.tauri.app/start/prerequisites/))

## Architecture (quick reference)

Same binary, three modes:
- Default: Tauri desktop UI
- `--mcp-server`: stdio JSON-RPC MCP server (Claude Code spawns this)
- `--hook`: UserPromptSubmit hook handler (Claude Code spawns this per message)

Storage:
- SQLite with FTS5 (WAL mode) at `~/.claude-memory-manager/memories.db`
- Override path via `CLAUDE_MEMORY_DB_DIR` env var

Files it manages in each `~/.claude*/`:
- `CLAUDE.md` — managed section with bootstrap prompt + save guidance
- `settings.json` — hook registration + MCP permission grant
- `.claude.json` — MCP server registration (via `claude mcp add`)

Everything outside the markers/keys is untouched.

## Verify downloads

Compare the SHA-256 checksums of your downloads against the values below (also in `checksums.sha256`):

```
e40a10a2938dc22a0cfe677e6bb1107e135d6a7b7bebd16aa0a5da35d7a3a55c  claude-memory-manager-0.1.0-macos-arm64.zip
233520188bcd61c9f27106be23742bbfd0974c23a758ddc3e216bf6a6ef58900  claude-memory-manager-0.1.0-macos-arm64.dmg
```

Check locally:

```bash
shasum -a 256 claude-memory-manager-0.1.0-macos-arm64.dmg
shasum -a 256 claude-memory-manager-0.1.0-macos-arm64.zip
```

## License

MIT — see [LICENSE](./LICENSE)
