# Daemon configuration

Two layers configure `cctui-daemon`, and they answer different questions.

## `~/.config/cctui/daemon.toml`

Machine identity and update policy, written by `cctui-daemon enroll`.

| Key | Meaning |
|-----|---------|
| `server_url` | The cctui server this machine connects to. |
| `machine_key` | Enrolment secret. `CCTUI_MACHINE_KEY` + `CCTUI_SERVER_URL` replace both for dispatched worker pods, which never run `enroll`. |
| `machine_id` | Filled in from `daemon_auth`. |
| `read_file_roots` | Extra roots the linked-file viewer may read from. |
| `channel` | `stable` or `beta`; see [release-channels.md](./release-channels.md). |

## `adapters_enabled` (server-side)

Per-machine, per-adapter rows held on the server and pushed down in the
`Reconcile` frame. They decide which adapters run and how. They are *server*
state: an operator sitting at the machine cannot edit them, which is why the
knobs whose failure mode is local get an environment override below.

Which adapters run follows the harness table (`cctui-proto`, served at
`GET /api/v1/harnesses`): a harness with `default_enabled` runs on every machine
unless a row disables it; a default-off harness runs only on machines with a
row enabling it. Rows are written by an admin through

```
GET    /api/v1/machines/{machine_id}/adapters
PUT    /api/v1/machines/{machine_id}/adapters/{adapter}   {"enabled": bool, "config": {...}}
DELETE /api/v1/machines/{machine_id}/adapters/{adapter}
```

or from Settings › Instance › Machine adapters. A `PUT` with only one field
keeps the other as stored; `DELETE` drops the row and the table default applies
again. Every write pushes a fresh `Reconcile` to the machine's daemon; running
sessions are left alone.

`config` is a JSON object the adapter interprets. Today the keys in use are the
claude-code `mode` and `supervise_daemon` below. Adapters that drive an external
binary read `bin` (path or name), `args` (extra argv) and `env` (names of
environment variables to pass through); an absent `bin` means the id itself.

## Environment overrides

Set these in the machine's service environment (`systemctl --user edit
cctui-daemon.service`, or the container's env).

| Variable | Effect |
|----------|--------|
| `CCTUI_DAEMON_CHANNEL` | Overrides `channel`. |
| `CCTUI_CLAUDE_SUPERVISE_DAEMON` | Overrides the claude-code adapter's `supervise_daemon`. |

### `CCTUI_CLAUDE_SUPERVISE_DAEMON`

Whether this machine's cctui-daemon may boot and supervise the on-demand
`claude daemon` at all. Accepts `1`/`true`/`yes`/`on` and `0`/`false`/`no`/`off`
(case-insensitive, trimmed); unset or empty leaves the server-side
`supervise_daemon` value alone, and an unparseable value is ignored with a
warning. It is the last word: it beats `adapters_enabled.config`.

**On (the default).** The adapter keeps a supervisor resident:

- with a usable service manager (systemd user manager, or launchd on macOS) it
  installs and runs `claude-daemon.service` / `dev.claude.daemon`, refreshes the
  unit in place when the template changes, and cycles it onto a new claude CLI
  when nothing is running;
- without one (worker containers: no `/run/systemd/system`, no user bus) it
  spawns `claude daemon run` as a detached child instead;
- either way, a missing `control.sock` triggers a rate-limited relaunch.

**Off.** cctui-daemon never starts, restarts or installs anything claude-side.
It only ever *connects* to whatever `control.sock` it finds. This is a real,
supported mode, and it is what the conformance and driver tests run in — but
nothing else will start the claude daemon for you, so something outside cctui
must keep it alive (your own unit, a `tmux` session, `claude daemon run` by
hand). While no supervisor answers, `list` and `dispatch` fail and the roster
flushes; the daemon logs it and retries, and recovers by itself as soon as a
socket appears. Sessions already adopted by a live claude daemon are untouched.

Turn it off when something else owns the claude daemon's lifecycle on that
machine, or to stop cctui writing a user unit on a box where you manage
services yourself.
