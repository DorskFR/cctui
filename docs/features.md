# Server features

A cctui server can run with whole categories switched off. The switch is a
TOML file named by `CCTUI_FEATURES_FILE`:

```toml
default = true          # state of every feature not listed below
[features]
machine_spawn = false
transcript_search = false
```

- No `CCTUI_FEATURES_FILE`: every feature is on.
- An unknown key, a value that is not a boolean, or an unreadable file: the
  server refuses to start and names the problem.
- A feature set to `true` whose requirement is off: the server refuses to
  start, naming both.
- A feature left unlisted whose requirement is off: it is off. The server logs
  it once at boot.
- The resolved set is logged at boot on one line (`features: on=[..] off=[..]`).

A feature that is off is not mounted. Its `/api/v1` routes and its routes
outside the API table answer 404, its background tasks do not start, and its
WebSocket messages are ignored. An off feature also wins over the environment
switch of the same integration (`CCTUI_PREVIEW_HOST`, `CCTUI_PLUGINS_DIR`,
`CCTUI_CCTUIVERSE`, `CCTUI_UPDATE_CHECK`, the Langfuse settings).

`deploy/features/core.toml` is the core deployment: every feature off. What
is left is token login and logout, users and keys, accounts and OAuth,
provider settings, redirects and model aliases, usage and limit reset,
dispatcher enrollment, dispatch, failed-dispatch records, the harness and
model catalogs, spawn memory, transcript ingest and read, the session list,
stats and events, conversation and live stream, message, interrupt,
set-model, resume, fork, kill and archive, manual approval, the gateway, tool
guard and hook policy, the completion webhook and transcript image blobs.
Enrolled machines, machine spawn and daemon self-update are off: workers are
the dispatched ones, running the binary their image ships.

## Capabilities

`GET /api/v1/capabilities` lists the always-on names plus the enabled
switchable features in `features`, and every switchable feature the server
knows in `switchable`. A client treats a feature as on when it is in
`features`, or when it is missing from `switchable` (a server older than the
feature).

## Features

| Feature | Requires | Covers |
|---|---|---|
| `machine_spawn` | | User-enrolled machines and spawning on them: enroll and deenroll, `POST /sessions/spawn`, the spawn directory history, hook registration (`/sessions/register`, `/sessions/{id}/deregister`), machine status, events, resources and harness pins, and machine administration (`/admin/machines/**`, `/admin/users/{id}/machines`). Off, the server only runs dispatched workers and keeps no host resource snapshots. |
| `files` | `machine_spawn` | Reading files, listing directories and git info on a machine (`/machines/{id}/fs/*`, `/sessions/{id}/linked-file*`). |
| `uploads` | `machine_spawn` | Attaching files to a session, the session attachment list, upload caps. |
| `drafts` | `machine_spawn` | Unsent composer drafts and draft sessions (launch, discard, cancel). |
| `profiles` | `machine_spawn` | Spawn profiles. |
| `terminal` | `machine_spawn` | The live terminal view (`watch_terminal` WebSocket message). |
| `transcript_search` | | Search across sessions and find-in-conversation, and the search backfill task. |
| `child_sessions` | | Sessions spawning sessions (`CctuiAgent`): the daemon's spawn, message, archive and completion routes, and the instance spawn limits. |
| `context` | | Rules and memories: the context library, a session's context, the agent context tool. The launch-time context pull stays mounted. |
| `plugins` | | Runtime plugins: install, settings, backend proxy, static files, boot init and the sync task. |
| `auto_approve` | | Per-session auto-approval of tool use. |
| `scheduling` | | Scheduled messages and scheduled launches, and their tasks. |
| `automation` | | Keep-alive, auto-resume, auto-redeem and follow-ups, their tasks, the follow-up thread view, and the external trigger endpoint. |
| `rooms` | | Rooms, peers and agent-to-agent messages. |
| `voice` | | Speech synthesis and transcription, the voice WebSocket, agent voice notes, the speech settings. |
| `previews` | | Dev-server previews: preview tickets, the internal preview leg and the preview sweep. |
| `federation` | | Links to other cctui servers (cctuiverse): management routes, the server-to-server wire and the outbox task. |
| `device_login` | | Logging a device in by approving a code. |
| `passkeys` | | Passkey enrollment and passkey login. |
| `self_update` | | Server self-update, daemon self-update (`/manifest/daemon`, `/daemon/binary/{target}`), harness auto-update, update hooks, the release probe and changelog. |
| `account_sharing` | | Account pools and sharing accounts or resources with other users. |
| `observability` | | Langfuse cost views and the browser OTLP relay. |
| `library` | | Saved prompts and the skill registry. |
| `organising` | | Labels, bookmarks, pinned sessions and pinned messages. Off, a spawn or draft ignores the labels it carries and the label-intent sweep does not run. |

Every feature gates at least one route, task or message. A new feature that
gates nothing must be listed as announcement-only in the gate test in
`crates/cctui-server-workspace/src/lib.rs`, with the reason here.

## Classifying a route

Every entry of `cctui_proto::api::routes::ROUTES` carries `feature`: `None`
for core, `Some(Feature::..)` for a switchable one. All methods of one path
share a feature. A core route must also be listed in
`crates/cctui-server-workspace/src/api_routes/core_routes.txt`; the route
tests fail until it is. Routes outside the table are classified where they are
mounted, in `outer_routes` in `crates/cctui-server-workspace/src/lib.rs`.
