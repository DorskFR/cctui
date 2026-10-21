---
title: Configuration
description: Server environment variables, the daemon config file, per-machine adapters and release channels.
---

## Server

`cctui-server` is configured by environment. The local stack sets these in `deploy/local/.env` and `deploy/local/docker-compose.yaml`.

| Variable | Meaning |
|---|---|
| `DATABASE_URL` | PostgreSQL connection string. The server migrates on start. |
| `CCTUI_ADMIN_TOKENS` | Comma-separated admin bearer tokens. The server refuses to start without one. |
| `CCTUI_VAULT_KEY` | Key for the encrypted credential vault (ChaCha20-Poly1305). A fresh install refuses to start without a valid key. |
| `CCTUI_HOST`, `CCTUI_PORT` | Bind address and port (default `8700`). |
| `CCTUI_EXTERNAL_URL` | Public URL of the server, used in enrolment commands and invite links. |
| `CCTUI_ALLOWED_ORIGINS` | Origins allowed to open the WebSocket. Foreign origins are refused. |
| `CCTUI_RP_ID` | WebAuthn relying-party id for passkeys. |
| `CCTUI_TRUSTED_PROXY_HOPS` | How many reverse proxies sit in front, for client IPs. |
| `CCTUI_PLUGINS_DIR` | Optional read-only plugin directory (dev and the local stack). |
| `CCTUI_NTFY_URL`, `CCTUI_NTFY_TOKEN` | ntfy endpoint for phone push. ntfy is the only push transport today. |
| `CCTUI_UPSTREAM_ALLOWED_HOSTS` | Internal or plain-`http` hosts a compatible-endpoint account may use (also editable in Settings › Instance). |
| `CCTUI_SPAWN_MAX_CHILDREN`, `CCTUI_SPAWN_MAX_DEPTH`, `CCTUI_SPAWN_MAX_TREE_BUDGET_USD` | Instance-wide subagent limits. Unset means no limit. |
| `CCTUI_DISPATCHERS` | JSON array of HTTP dispatcher registrations. Kube dispatchers enroll instead. |
| `CCTUI_CCTUIVERSE` | Cross-instance session links, on by default; `0` turns them off. |
| `CCTUI_GITHUB_TOKEN` | Raises the GitHub API rate limit for release lookups. |
| `CCTUI_SESSION_ARCHIVE_TTL_HOURS`, `CCTUI_EPHEMERAL_MACHINE_TTL_HOURS` | Retention for archived sessions and stale ephemeral machines. |
| `CCTUI_DB_MAX_CONNECTIONS`, `CCTUI_DB_MIN_CONNECTIONS`, `CCTUI_DB_ACQUIRE_TIMEOUT_SECS`, `CCTUI_DB_SLOW_QUERY_MS` | Pool and slow-query logging. |
| `CCTUI_UPLOAD_BODY_LIMIT` | Maximum attachment upload size. |

Most instance settings (upstream hosts, spawn limits, upload caps, harness health, plugins) are also editable at runtime under **Settings › Instance** and apply to every replica within 30 seconds.

## Daemon

`~/.config/cctui/daemon.toml` is written by `cctui-daemon enroll`:

| Key | Meaning |
|---|---|
| `server_url` | The server this machine connects to. |
| `machine_key` | Enrolment secret. Worker pods get `CCTUI_SERVER_URL` and `CCTUI_MACHINE_KEY` from the dispatcher instead and never run `enroll`. |
| `read_file_roots` | Extra roots the linked-file viewer may read. |
| `channel` | `stable` (default) or `beta`. `CCTUI_DAEMON_CHANNEL` overrides it. |

`CCTUI_DAEMON_AUTOUPDATE=0` disables the update poller (same as `cctui-daemon run --no-auto-update`).

### Per-machine adapters

Which harnesses run on a machine is server-side state, edited under **Settings › Execution › Machine adapters** or through `PUT /api/v1/machines/{id}/adapters/{adapter}`. Each row has `enabled` and a JSON `config` the adapter interprets (for example the Claude Code execution mode). Every write pushes a fresh `Reconcile` frame to the daemon; running sessions are left alone.

## Release channels

The tag decides the channel: `vX.Y.Z` is stable, `vX.Y.Z-beta.N` is beta. Images get `:X.Y.Z` plus `:latest`, or `:X.Y.Z-beta.N` plus `:beta`. Daemons and the TUI follow the version their server runs and never install a beta unless they opted in. Both channels go through the same CI gate and every asset is minisign-signed.

## Local stack overrides

`deploy/local/.env` accepts `CCTUI_UI_PORT`, `CCTUI_SERVER_PORT`, `CCTUI_BIND_ADDR` (default `127.0.0.1`), `CCTUI_SERVER_IMAGE` and `CCTUI_UI_IMAGE` (default `ghcr.io/dorskfr/cctui:latest` and `ghcr.io/dorskfr/cctui-ui:latest`).
