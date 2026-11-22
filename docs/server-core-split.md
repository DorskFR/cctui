# Server core and workspace split

There is one server, `cctui-server`, shipped as the `ghcr.io/dorskfr/cctui`
image. Its execution engine lives in a shared library; the interactive
workspace is assembled on top of it. What a running server offers is decided by
its feature file, not by a separate binary.

## Crates

- `cctui-server-kernel`: the delivery bus, auth and route authorization,
  configuration, errors, the schema runner, adapter normalization, outbound
  HTTP, lifecycle events, the session registry and pending commands, the
  token/session/spawn-capability stores, peer delivery and rate limits, the
  `core` schema stream, and the types other domains bind against
  (`accounts_types`, `sessions_types`, `extras_types`, `dispatch_types`, `blobs_types`, `notify_types`, `search_types`, `search_text`). It depends on no other server crate; domain
  crates reach the host through `KernelContext` and their own context traits.
- `cctui-server-accounts`: identity, provider accounts and credentials, pools,
  usage, limit resets, passkeys and the provider gateway, with their HTTP
  handlers. It depends only on the kernel and reaches the host through
  `AccountsHost` and the account context traits.
- `cctui-server-dispatch`: dispatchers and dispatch records, dispatch policy,
  worker identity, machine launches and enrollment, child sessions and their
  limits, and the dispatcher/manifest targets, with their HTTP handlers. It
  depends only on the kernel and reaches the host through `DispatchCore` and
  the `hooks` traits (accounts, sessions, notify, presence).
- `cctui-server-extras`: Langfuse tracing, storage stats, plugins and their
  proxy, skills, previews, WS presence, rooms and the cctuiverse federation
  with peer messaging, with their HTTP handlers. It depends only on the kernel
  and reaches the host through `ExtrasContext` and the `hooks` traits
  (sessions, blobs).
- `cctui-server-notify`: notifications (store, routing, in-app, ntfy, email and
  Web Push channels, producers, expiry, retention), the session user-action
  list, with their HTTP handlers. It depends only on the kernel and reaches the
  host through `NotifyContext` and the `hooks` traits (sessions, dispatch).
- `cctui-server-sessions`: the session domain: daemon transport, ingest and
  reconcile, run lifecycle, spawn and child sessions, follow-ups, keep-alive,
  presets, the inbox and scheduled messages, webhooks, context and the
  session HTTP/WebSocket handlers. It depends only on the kernel and reaches
  the host through `SessionsContext` and the `ports` traits (accounts, blobs,
  dispatch, extras, notify, search, host).
- `cctui-server`: the host and the executable. It owns application state
  (`AppState`, with the execution caches in `CoreState`), implements every
  domain context, mounts the route tables, runs the `workspace` schema stream
  and holds the cross-domain contract tests.

Workers, dispatchers, the daemon and the TUI are the same for every
installation.

## Ownership

| Area | Core | Workspace |
| --- | --- | --- |
| Identity | Users, service identities, API keys, scopes, revocation, ownership, audit | Interactive login adapters and identity screens |
| Accounts | Encrypted provider credentials, OAuth setup/refresh, pools, model routing | Account dashboards and historical charts |
| Gateway | Provider proxying, ephemeral tokens, limits, budget enforcement | Optional tracing exporters and reporting |
| Targets | Machine/dispatcher enrollment, daemon transport, dispatcher backends | Filesystem browser, interactive terminal, previews |
| Launch | Machine/dispatcher launch, harness/model/configuration selection | Multipart uploads, drafts, interactive context resolution |
| Policy | Dispatch grants, permission ceilings, tool/network policy, child-spawn limits | Human approval/question/plan interfaces |
| Lifecycle | Stable run identity, acknowledgment, status, cancellation, result/failure | Conversation navigation and organisation |
| Events | Lifecycle, accounting, bounded diagnostic records | Full transcript archive, search, images, filesystem links |
| Inputs | Prompt/configuration references and worker context packs | Server attachment storage, skill editors, shared memory |
| Operations | Health/readiness, essential metrics, configuration, migrations, cleanup | Voice, plugins, rooms, federation |

Unattended runs never wait indefinitely for a human: a blocked run reaches a
terminal outcome with an explicit resolution path, and policy enforcement still
applies. Removing approval screens does not grant permission.

## Feature file

`CCTUI_FEATURES_FILE` names a TOML file that switches workspace features off;
with no file every feature is on. `deploy/features/core.toml` turns every
switchable feature off, leaving the execution API: that is "core". Routes,
background tasks, WebSocket commands, launch paths, the daemon's agent tools and
the web UI follow the resolved set, and `GET /api/v1/capabilities` reports it.
See [features](features.md) for the file format and what each feature carries.

## Database

The schema is split into packages, each a forward-only stream of numbered SQL
files under `crates/<package>/schema/`, registered in that crate's `schema.rs`
(or `schema/<stream>/`, registered in `src/schema/<stream>.rs`): `core` in
`cctui-server-kernel` (`schema/core/`), `workspace` in `cctui-server`
and `workspace_search` in `cctui-server-search`, run by
the shared runner in `cctui-server-kernel`. The
legacy `migrations/` stream is frozen at 191; `scripts/check-migrations.sh`
rejects collisions.

The server provisions its schema at startup. `cctui-server init-db` does the
same, needing only `DATABASE_URL`, and exits; it provisions a fresh test
database for `cargo test`. On a database created from the legacy stream it runs
the historical migrator, then adopts the package baselines. On a database whose
recorded origin is core it installs the packages directly. Either way core and
workspace data stay in place.

Applied package SQL is checksum-checked, unknown package versions are refused,
and every replica serialises schema changes on the same advisory lock.
