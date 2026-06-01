# cctui

Web control plane for monitoring and driving Claude Code (and Codex) sessions across machines.

Watch conversations in real time, send input to running sessions, answer permission and interactive prompts, and spawn or dispatch agents on any enrolled machine — all from a single web UI.

> [!WARNING]
> cctui is an early-stage personal project under active development. Expect
> breaking changes, rough edges, and incomplete features. It is not stable and
> not yet recommended for production use.

## Architecture

```
                  cctui-daemon  <-->  Claude Code / Codex sessions
                       ^
cctui-server (Axum, PostgreSQL)  <-->  web UI
                       |
                  dispatchers  -->  ephemeral k8s worker pods
```

- **cctui-server** — Session registry, event store, full-text search, WebSocket hub, and REST API (`/api/v1`). Also serves the daemon-binary manifest used for self-updates.
- **cctui-daemon** — Long-lived per-machine daemon. Enrolls with the server, connects out over WebSocket, and spawns/observes local agent sessions through pluggable adapters (Claude Code, Codex). Self-updates from GitHub Releases.
- **web UI** — SvelteKit SPA (`webui/`): overview, live session view, spawn/dispatch, search & archive, and user/machine management.
- **cctui-admin** — CLI for managing users, machines, and archived sessions.

A terminal UI crate (`cctui-tui`) also exists but is currently work-in-progress.

## Features

- **Live session view** — Real-time conversation stream (text, tool calls, results) with markdown rendering, search-term highlighting, and a composer to reply to running agents.
- **Permission & interactive prompts** — Approve/deny tool-permission requests inline, toggle auto-approve, set per-session policies, and answer structured `AskUserQuestion` prompts as real forms.
- **Spawn & dispatch** — Start a session on any enrolled daemon (pick adapter, working dir, prompt, permission mode), or dispatch work to ephemeral Kubernetes worker pods.
- **Search & archive** — Full-text search across session transcripts (live and archived), with multi-select batch archiving and resume.
- **Multi-machine** — Enroll any number of machines; sessions, liveness, and token usage roll up into one view.
- **Adapter-agnostic** — Claude Code and Codex events normalize to a single canonical shape, so the UI has one renderer.

## Enrolling a machine

Install `cctui-daemon` on the target host (download the matching binary from GitHub Releases), then enroll it with a user token:

```sh
cctui-daemon enroll --server-url https://your-cctui-server --token <user-token> --name "$(hostname)"
cctui-daemon service install   # run it as a systemd user service
```

Create user tokens from the web UI's Users page (or with `cctui-admin`). Once
enrolled, the daemon connects to the server and the machine's sessions show up
in the UI. cctui does **not** modify your Claude Code configuration — the daemon
observes and spawns sessions directly.

## Local development

Prerequisites: Rust (nightly for fmt), Docker, PostgreSQL.

```sh
make setup          # start postgres, migrate, build
make run/server     # server on :8700
```

```sh
make webui/install  # bun install
make webui/dev      # Vite dev server
```

Default dev tokens are in the Makefile.

## License

[MIT](LICENSE)
