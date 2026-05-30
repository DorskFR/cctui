# cctui

Web control plane for monitoring and interacting with Claude Code (and Codex) sessions across machines.

Watch conversations in real time, send input to running sessions, and spawn/observe agents on any enrolled machine from a single web UI.

> [!WARNING]
> cctui is an early-stage personal project under active development. Expect
> breaking changes, rough edges, and incomplete features. It is not stable and
> not yet recommended for production use.

## Architecture

```
                  cctui-daemon  <-->  Claude Code / Codex sessions
                       ^
cctui-server (Axum, PostgreSQL)  <-->  web UI
```

- **cctui-server** — Session registry, event store, WebSocket hub, REST API (`/api/v1`). Also serves the daemon-binary manifest used for self-updates.
- **cctui-daemon** — Long-lived per-machine daemon. Enrolls with the server, connects out over WebSocket, and spawns/observes local agent sessions through pluggable adapters (Claude Code, Codex). Self-updates from GitHub Releases.
- **web UI** — SvelteKit SPA (`webui/`): overview, live session view, spawn, and user/machine management.
- **cctui-admin** — CLI for managing users and machines.

A terminal UI crate (`cctui-tui`) also exists but is currently work-in-progress.

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
