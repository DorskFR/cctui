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
outside the API table answer 404 (405 when the path also matches a kept route
of another method, as `POST /sessions/spawn` matches `GET /sessions/{id}`),
its background tasks do not start, and its WebSocket messages are ignored. An
off feature also wins over the environment switch of the same integration
(`CCTUI_PREVIEW_HOST`, `CCTUI_PLUGINS_DIR`, `CCTUI_CCTUIVERSE`,
`CCTUI_UPDATE_CHECK`, `CCTUI_EMOJI_ENDPOINT`, the Langfuse settings); a
`CCTUI_PLUGINS_DIR` or `CCTUI_EMOJI_ENDPOINT` set while its feature is off is
logged once at boot as ignored.

## Daemons

When a daemon connects, the server sends it the features that are off. A
server that announces nothing has every feature on. The daemon then:

- offers a session only the agent tools whose feature is on: `child_sessions`
  for agent spawn and archive, `rooms` for peers, send, history and room,
  `voice` for speak, `context` for context. Usage and user actions are always
  offered. A preview open or close call is answered with an error while
  `previews` is off.
- refuses, on its own, every frame of a feature that is off, even if the
  server sends it:

  | Frames | Refused when off |
  |---|---|
  | Directory listing, git info, file read | `files` |
  | Staging files into a session | `uploads` and `context` both |
  | Spawn, fork and resume of a session without a parent | `machine_spawn` |
  | Spawn of a session with a parent | `child_sessions` |
  | Terminal watch | `terminal` |
  | Update hook, harness update policy, harness drain and restart | `self_update` |
  | Preview requests, chunks, aborts and open or close replies | `previews` |

  A refused request gets a failed reply naming the feature; a refused command
  that carries a command id gets a failed command result; anything else is
  dropped. Messages, interrupts, kills, approvals, model changes and the
  other session commands belong to no feature and are always served.
- holds every feature-owned frame that arrives before the announcement, and
  every frame behind it so the order is kept, then judges them on the
  announcement. A server that announces nothing within 3 seconds of the
  connection (or once 1024 frames are waiting) has its held frames served as
  every feature on.

`deploy/features/core.toml` is the core deployment ([operator guide](./core.md)): every feature off. What
is left is token login and logout, users and keys, accounts and OAuth,
provider settings, redirects and model aliases, usage and limit reset,
dispatcher enrollment, dispatch, failed-dispatch records, the harness and
model catalogs, transcript ingest and read, the session list, stats and
events, conversation and live stream, message, interrupt, set-model, kill and
archive, manual approval, the gateway for per-session credentials, tool guard
and hook policy, the completion webhook and transcript image blobs. Enrolled
machines, machine spawn, fork, resume, account service tokens and daemon
self-update are off: workers are the dispatched ones, running the binary
their image ships, and the only way to start an agent is
`POST /sessions/dispatch`.

## Capabilities

`GET /api/v1/capabilities` lists the always-on names plus the enabled
switchable features in `features`, and every switchable feature the server
knows in `switchable`. A client treats a feature as on when it is in
`features`, or when it is missing from `switchable` (a server older than the
feature).

## Features

| Feature | Requires | Covers |
|---|---|---|
| `machine_spawn` | | User-enrolled machines and starting agents outside dispatch: enroll and deenroll, `POST /sessions/spawn`, fork and resume (`/sessions/{id}/fork`, `/sessions/{id}/resume`), the spawn directory history and spawn memory, hook registration (`/sessions/register`, `/sessions/{id}/deregister`), machine status, events, resources and harness pins, and machine administration (`/admin/machines/**`, `/admin/users/{id}/machines`). Off, the server only runs dispatched workers and keeps no host resource snapshots. |
| `files` | `machine_spawn` | Reading files, listing directories and git info on a machine (`/machines/{id}/fs/*`, `/sessions/{id}/linked-file*`). |
| `uploads` | `machine_spawn` | Attaching files to a session, the session attachment list, upload caps. |
| `drafts` | `machine_spawn` | Unsent composer drafts and draft sessions (launch, discard). |
| `profiles` | `machine_spawn` | Spawn profiles. |
| `terminal` | `machine_spawn` | The live terminal view (`watch_terminal` WebSocket message). |
| `transcript_search` | | Search across sessions and find-in-conversation, the search backfill task, chunking transcripts on ingest, and the `pg_trgm` and `btree_gin` schema step (see [Postgres requirements](#postgres-requirements)). |
| `child_sessions` | | Sessions spawning sessions (`CctuiAgent`): the daemon's spawn, message, archive and completion routes, and the instance spawn limits. |
| `context` | | Rules and memories: the context library, a session's context, the agent context tool. The launch-time context pull stays mounted. Off, dispatch, session start and fork record only who a launch belongs to, never context content. |
| `plugins` | | Runtime plugins: install, settings, backend proxy, static files, boot init and the sync task. |
| `auto_approve` | | Per-session auto-approval of tool use. |
| `scheduling` | | Scheduled messages and scheduled launches (schedule and cancel), and their tasks. |
| `automation` | | Keep-alive, auto-resume, auto-redeem and follow-ups, their tasks, the follow-up thread view, and the external trigger endpoint. |
| `rooms` | | Rooms, peers and agent-to-agent messages. |
| `voice` | | Speech synthesis and transcription, the voice WebSocket, agent voice notes, the speech settings. |
| `previews` | | Dev-server previews: preview tickets, the internal preview leg and the preview sweep. Off, the server drops every preview frame a worker sends: no registration is recorded and nothing is sent back. |
| `federation` | | Links to other cctui servers (cctuiverse): management routes, the server-to-server wire and the outbox task. |
| `device_login` | | Logging a device in by approving a code. |
| `passkeys` | | Passkey enrollment and passkey login, including the login screen's unauthenticated `/api/v1/auth/passkey/config` probe. |
| `self_update` | | Server self-update, daemon self-update (`/manifest/daemon`, `/daemon/binary/{target}`), harness auto-update, update hooks, the release probe and changelog. Off, the harness report and update-hook flag a heartbeat carries are ignored and no harness policy is sent. |
| `account_sharing` | | Account pools and sharing accounts or resources with other users. |
| `observability` | | Langfuse cost views and the browser OTLP relay. |
| `library` | | Saved prompts and the skill registry, and the boot sweep of orphaned skill files. |
| `organising` | | Labels, bookmarks, pinned sessions and pinned messages, and the session emoji picker (`CCTUI_EMOJI_ENDPOINT`, which sends session titles to an external model). Off, a spawn or draft ignores the labels it carries, session start claims no label intent and the label-intent sweep does not run. |
| `service_tokens` | | Account service tokens (`/accounts/{id}/service-tokens`): long-lived gateway credentials not tied to a session. Off, the gateway refuses a service token exactly as it refuses an unknown one, so tokens minted earlier stop working. |

Every feature gates at least one route, task or message. A new feature that
gates nothing must be listed as announcement-only in the gate test in
`crates/cctui-server-workspace/src/lib.rs`, with the reason here.

## Postgres requirements

| Mode | Extensions |
|---|---|
| `transcript_search` on (the default) | `pgcrypto`, `pg_trgm`, `btree_gin` |
| `transcript_search` off | `pgcrypto` |

The search objects (the two extensions and the trigram indexes on
`search_chunks` and `stream_events`) live in their own schema stream,
`workspace_search`, applied at boot only while `transcript_search` is on.

- A fresh database with search off is created without them: the server
  provisions it through the package streams, never the legacy migrations.
- Switching search on later applies the step at the next boot, and the
  backfill chunks the history ingested while it was off. The step builds both
  trigram indexes inside the boot transaction, holding a `SHARE` lock that
  blocks writes to `stream_events` and `search_chunks` until they are built.
- A database that already has the search objects is left as it is: the step
  creates only what is missing, and switching search off drops nothing.
- With search off, ingest writes no chunks, transcript rows and the
  session-ended row store an empty `search_text`, the backfill does not run
  and the search routes answer 404.

## Classifying a route

Every entry of `cctui_proto::api::routes::ROUTES` carries `feature`: `None`
for core, `Some(Feature::..)` for a switchable one. All methods of one path
share a feature. A core route must also be listed in
`crates/cctui-server-workspace/src/api_routes/core_routes.txt`; the route
tests fail until it is. Routes outside the table are classified where they are
mounted, in `outer_routes` in `crates/cctui-server-workspace/src/lib.rs`.
