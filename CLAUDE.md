# cctui — Claude Code Control TUI

Centralized server + TUI for managing Claude Code sessions across machines.

## Architecture

```
cctui-server (Axum, PostgreSQL)  ←→  cctui (TUI, default subcommand)
     ↑
cctui channel (Rust MCP server, same binary) ←→ Claude Code
```

- **cctui-server**: Session registry, event storage, WebSocket hub, policy check endpoint
- **cctui-tui**: Ratatui TUI — produces the `cctui` binary (default = TUI, `cctui channel` = MCP server)
- **cctui-proto**: Shared types (models, API, WebSocket messages)
- **cctui-channel**: Library crate for the MCP channel, re-exposed as the `cctui channel` subcommand

## Workspace

```
crates/
  cctui-proto/     # Shared types: Session, SessionStatus, AgentEvent, TuiCommand, etc.
  cctui-server/    # Axum server: routes/, ws.rs, registry.rs, auth.rs, config.rs, db.rs
  cctui-tui/       # Ratatui TUI; produces the `cctui` binary (bin name `cctui`)
  cctui-channel/   # MCP channel lib (invoked via `cctui channel`): mcp.rs, bridge.rs, transcript.rs, ...
  cctui-admin/     # Admin CLI (enroll, user management)
migrations/        # PostgreSQL schema (sessions, stream_events)
deploy/            # Dockerfile, K8s manifests
```

## How It Works

1. `make setup/claude` (with server running) writes `~/.claude.json` to invoke `cctui channel` as an MCP server + installs hooks into `~/.claude/settings.json`
2. Claude starts → spawns `cctui channel` as an MCP stdio subprocess
3. SessionStart hook POSTs to `cctui-server` (`/api/v1/hooks/session-start`); the channel polls and matches the session by `(machine_id, ppid)`
4. PreToolUse hook → `cctui-server` `/api/v1/check` for policy
5. Channel tails the transcript JSONL and forwards raw lines via `POST /api/v1/sessions/{sid}/transcript`
6. TUI messages queued on server → polled by channel → pushed as MCP notifications
7. Claude replies via the `cctui_reply` MCP tool → channel posts event → server broadcasts to TUI

## Key Files

- **Server entry**: `crates/cctui-server/src/main.rs` — route setup, reaper task
- **Session registration**: `crates/cctui-server/src/routes/sessions.rs`
- **Event ingestion**: `crates/cctui-server/src/routes/events.rs` (from channel)
- **Policy check**: `crates/cctui-server/src/routes/check.rs` (PreToolUse, currently allow-all)
- **WebSocket handlers**: `crates/cctui-server/src/ws.rs` (agent stream + TUI fan-out)
- **TUI app state**: `crates/cctui-tui/src/app.rs`
- **TUI views**: `crates/cctui-tui/src/views/sessions.rs`, `conversation.rs`
- **Proto types**: `crates/cctui-proto/src/models.rs`, `api.rs`, `ws.rs`
- **Channel entry**: `crates/cctui-channel/src/lib.rs` — `pub async fn run()`, invoked by `cctui channel`
- **Channel MCP**: `crates/cctui-channel/src/mcp.rs` — stdio JSON-RPC, `cctui_reply` tool, capability handshake
- **Channel bridge**: `crates/cctui-channel/src/bridge.rs` — REST client to cctui-server
- **Project status & gaps**: `STATUS.md`

## Development

```bash
make setup          # start postgres, migrate, build
make run/server     # starts server (default tokens: dev-agent, dev-admin)
make run/tui        # starts TUI (connects to localhost:8700)
make setup/claude   # install hooks (server must be running)
make test/unit      # unit tests (no DB needed)
make fmt            # format (cargo +nightly fmt + biome)
make lint           # clippy with deny warnings
```

Env defaults in Makefile: `DATABASE_URL`, `CCTUI_AGENT_TOKENS=dev-agent`, `CCTUI_ADMIN_TOKENS=dev-admin`, `CCTUI_URL=http://localhost:8700`.

## Conventions

- Rust 2024 edition, clippy pedantic + nursery (workspace lints in root Cargo.toml)
- `cargo +nightly fmt --all` for formatting (rustfmt.toml: 100 width, module imports)
- Lefthook pre-commit: fmt, clippy, check, biome
- Commits: conventional commits (`feat:`, `fix:`, `chore:`)
- Hooks go in `~/.claude/settings.json` (NOT settings.local.json, NOT managed-settings.json)
- Claude's Stop hook fires per-turn — don't use for session cleanup (reaper handles it)

## Tracking

YouTrack project: CCT at https://youtrack.dorsk.dev/issues/CCT
