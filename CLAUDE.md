# cctui — Claude Code Control TUI

Centralized server + TUI for managing Claude Code sessions across machines.

## Architecture

```
cctui-server (Axum, PostgreSQL)  ←→  cctui-tui (Ratatui)
     ↑
Claude Code hooks + streamer.py
```

- **cctui-server**: Session registry, event storage, WebSocket hub, policy check endpoint
- **cctui-tui**: Terminal UI — session list, conversation viewer, chat input
- **cctui-proto**: Shared types (models, API, WebSocket messages)
- **cctui-shim**: stdin-to-WebSocket relay (not used in current flow)

## Workspace

```
crates/
  cctui-proto/     # Shared types: Session, SessionStatus, AgentEvent, TuiCommand, etc.
  cctui-server/    # Axum server: routes/, ws.rs, registry.rs, auth.rs, config.rs, db.rs
  cctui-tui/       # Ratatui TUI: views/, widgets/, app.rs, client.rs, theme.rs
  cctui-shim/      # Shim binary (stdin relay, not actively used)
scripts/
  streamer.py      # Tails Claude transcript JSONL, POSTs events to server
  bootstrap.sh.tpl # Template for SessionStart hook script
  setup.py.tpl     # Template for setup endpoint output
migrations/        # PostgreSQL schema (sessions, stream_events)
deploy/            # Dockerfile, K8s manifests
```

## How It Works

1. `make setup/claude` (with server running) installs hooks into `~/.claude/settings.json`
2. Claude starts → `SessionStart` hook runs `~/.cctui/bin/bootstrap.sh`
3. Bootstrap reads Claude's session_id from stdin, registers with server
4. Bootstrap spawns `streamer.py` which tails the transcript JSONL file
5. `PreToolUse` HTTP hook fires on every tool call → server stores + broadcasts
6. Streamer POSTs parsed conversation events (user messages, assistant text, tool calls, results)
7. TUI connects via REST + WebSocket, fetches history, subscribes to live events

## Key Files

- **Server entry**: `crates/cctui-server/src/main.rs` — route setup, reaper task
- **Session registration**: `crates/cctui-server/src/routes/sessions.rs`
- **Event ingestion**: `crates/cctui-server/src/routes/events.rs` (from streamer)
- **Policy check**: `crates/cctui-server/src/routes/check.rs` (PreToolUse, currently allow-all)
- **WebSocket handlers**: `crates/cctui-server/src/ws.rs` (agent stream + TUI fan-out)
- **TUI app state**: `crates/cctui-tui/src/app.rs`
- **TUI views**: `crates/cctui-tui/src/views/sessions.rs`, `conversation.rs`
- **Proto types**: `crates/cctui-proto/src/models.rs`, `api.rs`, `ws.rs`
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
