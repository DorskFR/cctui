# cctui

Terminal control plane for monitoring and interacting with Claude Code sessions across machines.

Watch conversations in real time, send messages to running sessions, and manage multiple Claude instances from a single TUI.

## Architecture

```
cctui-server (Axum, PostgreSQL)  <-->  cctui-tui (Ratatui)
     ^
cctui-channel (Bun/TS MCP server) <--> Claude Code
```

- **cctui-server** — Central hub. Stores sessions and events, broadcasts via WebSocket.
- **cctui-tui** — Terminal UI. Session list, conversation viewer with inline diffs, chat input.
- **cctui-channel** — MCP subprocess spawned by Claude Code. Tails the session transcript and bridges events to the server. Delivers TUI messages back to Claude.

## Status

<img width="1674" height="901" alt="Screenshot 2026-03-30 at 16 32 44" src="https://github.com/user-attachments/assets/2994daa5-9e12-4cf7-818b-3e574357087e" />

Working locally. Not yet deployed to production. See [YouTrack](https://youtrack.dorsk.dev/issues/CCT) for open issues.

What works:
- Session registration and live event streaming
- Conversation view with markdown rendering, syntax-highlighted diffs, scrollbar
- Bidirectional messaging (TUI to Claude and back via MCP channel)
- Hook-based session discovery (SessionStart, PreToolUse)

What doesn't yet:
- Production deployment (CCT-36)
- Multi-user auth / session scoping
- Raw JSONL archival (CCT-37)

## Install (remote machine)

Once a release is published, a one-line installer is available:

```sh
curl -fsSL https://cctui.dorsk.dev/install.sh | CCTUI_TOKEN=... sh
```

The script detects OS/arch, downloads the `cctui` binary from GitHub Releases
and the channel bundle from the server, writes the MCP entry to `~/.claude.json`,
and wires hooks into `~/.claude/settings.json`. Set `CCTUI_URL` to point at a
different server. See `scripts/install.sh`.

## Local Development

Prerequisites: Rust (nightly for fmt), Bun, Docker, PostgreSQL.

```sh
make setup          # start postgres, run migrations, build everything
make run/server     # start server on :8700
make run/tui        # start TUI (connects to localhost:8700)
make setup/claude   # configure Claude Code hooks + MCP channel
```

Then start Claude Code with:
```sh
claude --dangerously-load-development-channels server:cctui
```

Default dev tokens are in the Makefile.

## License

[WTFPL](LICENSE)
