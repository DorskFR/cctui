# Server core and workspace split

There is one server, `cctui-server`, shipped as the `ghcr.io/dorskfr/cctui`
image. Its execution engine lives in a shared library; the interactive
workspace is assembled on top of it. What a running server offers is decided by
its feature file, not by a separate binary.

## Crates

- `cctui-server-core`: identity, provider accounts, gateway, targets, launch,
  policy, run lifecycle and their persistence. It does not depend on the
  workspace and compiles and tests on its own, without search, image codecs,
  WebAuthn or plugin startup.
- `cctui-server-workspace`: interactive HTTP/WebSocket handlers, application
  state, workspace storage and the full-host assembly, built on core.
- `cctui-server`: the executable; it starts the workspace host.

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
files under `crates/<package>/schema/`, registered in that crate's `schema.rs`:
`core` in `cctui-server-core` and `workspace` in `cctui-server-workspace`. The
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
