# cctui

Terminal control plane for monitoring and interacting with Claude Code sessions across machines.

Watch conversations in real time, send messages to running sessions, and manage multiple Claude instances from a single TUI.

## Architecture

```
cctui-server (Axum, PostgreSQL)  <-->  cctui (TUI, Ratatui)
     ^
cctui channel (MCP subprocess)   <-->  Claude Code
```

- **cctui-server** — Session registry, event store, WebSocket hub, policy endpoint.
- **cctui** — TUI binary. Default subcommand is the TUI; `cctui channel` runs the MCP server.
- **cctui channel** — Spawned by Claude Code as an MCP stdio subprocess. Tails the transcript, forwards events, delivers TUI messages back.

## Status

<img width="1674" height="901" alt="cctui screenshot" src="https://github.com/user-attachments/assets/2994daa5-9e12-4cf7-818b-3e574357087e" />

Works end-to-end locally and on a single remote deployment. Single-user.

Working:
- Session registration and live event streaming
- Conversation view with markdown, syntax-highlighted diffs, scrollbar
- Bidirectional messaging (TUI ⇄ Claude via MCP channel)
- Hook-based session discovery (SessionStart, PreToolUse)

Not yet:
- Multi-user auth / session scoping
- Policy engine (PreToolUse currently allow-all)
- Raw JSONL archival

## Install (remote machine)

```sh
curl -fsSL https://cctui.dorsk.dev/install.sh | CCTUI_TOKEN=... sh
```

Detects OS/arch, downloads the `cctui` binary from GitHub Releases, writes the MCP entry to `~/.claude.json`, and wires hooks into `~/.claude/settings.json`. Set `CCTUI_URL` to point at a different server. See `scripts/install.sh`.

## Local development

Prerequisites: Rust (nightly for fmt), Docker, PostgreSQL.

```sh
make setup          # start postgres, migrate, build
make run/server     # server on :8700
make run/tui        # TUI (connects to localhost:8700)
make setup/claude   # install MCP entry + hooks for local Claude Code
```

Default dev tokens are in the Makefile. See `CLAUDE.md` for crate layout and conventions.

## License

[WTFPL](LICENSE)
