---
title: Command line
description: cctui-daemon, cctui (terminal client) and cctui-admin subcommands.
---

All three binaries are published on [GitHub Releases](https://github.com/DorskFR/cctui/releases) for Linux and macOS, and are minisign-signed.

## cctui-daemon

Runs on every machine in the fleet. Config lives in `~/.config/cctui/daemon.toml`.

| Command | What it does |
|---|---|
| `enroll [user@host] --server-url URL --token TOKEN [--name NAME] [--kind persistent\|ephemeral]` | Mint a machine key and write the config. With an ssh target: one-shot remote install (push binary, enroll, install the service, wait for the machine to connect). Idempotent. |
| `run [--no-auto-update]` | Connect to the server and supervise adapters. This is what the service runs. |
| `status` | Print the resolved configuration with the machine key redacted, including the channel in effect. |
| `update` | Check for a newer release, swap the binary in place and re-exec the running daemon with live sessions kept. |
| `doctor [--grant] [--yes]` | Probe folder and network access as the daemon sees them, and the `claude` CLI versus `claude daemon` version split. On macOS, relaunch to re-ask privacy grants. |
| `service install\|uninstall\|restart\|status` | Manage the systemd (Linux) or launchd (macOS) user service. `restart` ends live codex and opencode sessions; `update` does not. |
| `preview open --port PORT [--session ID]` / `preview close --port PORT` | Expose a local dev server as an owner-only preview URL through the server. Run from inside an agent session. |
| `selfcheck` | Exercise start-up paths that `--version` skips and exit 0. |
| `--version` | Prints the build's version and channel, e.g. `cctui-daemon 0.26.1 (stable)`. |

Internal subcommands (`ask-hook`, `mcp-agent`, `mcp-wait`, `whip-stop-hook`) are registered by the daemon in per-session hook and MCP configs; you do not run them yourself.

Environment: `CCTUI_SERVER_URL` and `CCTUI_MACHINE_KEY` replace the config file for worker pods; `CCTUI_DAEMON_CHANNEL=beta` opts into the beta channel; `CCTUI_DAEMON_AUTOUPDATE=0` disables the update poller.

## cctui (terminal client)

| Command | What it does |
|---|---|
| `cctui [--filter QUERY]` | Open the session list, optionally with a search applied, e.g. `label:backend`. |
| `cctui login [--server URL] [--key [KEY]]` | Device-code login approved in the browser, stored in `~/.config/cctui/user.json`. `--key` uses an existing key, read from stdin when no value is given. |
| `cctui logout [--revoke]` | Forget the stored credential, optionally revoking it server-side. |
| `cctui open SESSION_ID [--seq N]` | Start on one session's conversation; an archived session is fetched by id. |
| `cctui diagnose SESSION_ID` | Print everything the daemon and server know about a session, one dated and sourced line per fact. |
| `cctui update [--force]` | Re-download the release the server runs and re-apply settings. |

`CCTUI_URL` and `CCTUI_TOKEN` override the stored credential; `CCTUI_CHANNEL=beta` lets a stable build update onto a beta server.

## cctui-admin

Admin operations against a server. `--server` (or `CCTUI_URL`) and `--token` (or `CCTUI_ADMIN_TOKEN`); `enroll` takes a user token.

| Command | What it does |
|---|---|
| `user create NAME [--save]` | Create a user and print the key; `--save` writes it to `~/.config/cctui/user.json`. |
| `user list` / `user revoke ID` / `user rotate ID` / `user machines ID` | Manage users and list a user's machines. |
| `machine revoke ID` / `machine rotate ID` / `machine rename ID [NAME\|--clear]` | Manage machine keys and display names. |
| `enroll [--hostname NAME] [--user-token TOKEN]` | Enroll this host with a user token (or `CCTUI_USER_TOKEN`) and write `~/.config/cctui/machine.json`. |
| `usage [--json]` | This instance's account usage windows, the numbers `/metrics` exports, as a table or a stable JSON envelope for a status bar. |
| `skills list` / `skills push NAME [--root DIR] [--version LABEL]` | Sync `~/.claude/skills/<name>/` bundles with the server. |
