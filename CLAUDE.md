# cctui — Claude Code Control TUI

Centralized server + TUI for managing Claude Code sessions across machines.

## Architecture

```
cctui-server (Axum, PostgreSQL)  ←→  cctui-tui (Ratatui)
     ↑
cctui-channel (Bun/TS MCP server) ←→ Claude Code
```

- **cctui-server**: Session registry, event storage, WebSocket hub, policy check endpoint
- **cctui-tui**: Terminal UI — session list, conversation viewer, chat input
- **cctui-proto**: Shared types (models, API, WebSocket messages)
- **cctui-channel**: MCP channel server for bidirectional Claude ↔ TUI messaging

## Workspace

```
crates/
  cctui-proto/     # Shared types: Session, SessionStatus, AgentEvent, TuiCommand, etc.
  cctui-server/    # Axum server: routes/, ws.rs, registry.rs, auth.rs, config.rs, db.rs
  cctui-tui/       # Ratatui TUI: views/, widgets/, app.rs, client.rs, theme.rs
channel/
  src/             # MCP channel server: index.ts, mcp.ts, hooks.ts, bridge.ts, transcript.ts
migrations/        # PostgreSQL schema (sessions, stream_events)
deploy/            # Dockerfile, K8s manifests
```

## How It Works

1. `make setup/claude` (with server running) configures `.mcp.json` + installs hooks into `~/.claude/settings.json`
2. Claude starts → spawns cctui-channel as MCP subprocess via `.mcp.json`
3. SessionStart hook → channel HTTP endpoint → registers session with server, starts transcript tailing
4. PreToolUse hook → channel HTTP endpoint → proxied to server for policy check
5. Transcript events streamed to server via channel bridge → broadcast to TUI
6. TUI messages queued on server → polled by channel → pushed as MCP notifications
7. Claude replies via `cctui_reply` tool → posted to server → broadcast to TUI

## Key Files

- **Server entry**: `crates/cctui-server/src/main.rs` — route setup, reaper task
- **Session registration**: `crates/cctui-server/src/routes/sessions.rs`
- **Event ingestion**: `crates/cctui-server/src/routes/events.rs` (from channel)
- **Policy check**: `crates/cctui-server/src/routes/check.rs` (PreToolUse, currently allow-all)
- **WebSocket handlers**: `crates/cctui-server/src/ws.rs` (agent stream + TUI fan-out)
- **TUI app state**: `crates/cctui-tui/src/app.rs`
- **TUI views**: `crates/cctui-tui/src/views/sessions.rs`, `conversation.rs`
- **Proto types**: `crates/cctui-proto/src/models.rs`, `api.rs`, `ws.rs`
- **Channel entry**: `channel/src/index.ts` — MCP channel server entry point
- **Channel MCP**: `channel/src/mcp.ts` — channel capability + reply tool
- **Channel hooks**: `channel/src/hooks.ts` — HTTP hook receiver
- **Channel bridge**: `channel/src/bridge.ts` — REST client to cctui-server
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
- Commits: `--no-gpg-sign`, conventional commits (`feat:`, `fix:`, `chore:`)
- Hooks go in `~/.claude/settings.json` (NOT settings.local.json, NOT managed-settings.json)
- Claude's Stop hook fires per-turn — don't use for session cleanup (reaper handles it)

## Tracking

YouTrack project: CCT at https://youtrack.dorsk.dev/issues/CCT
