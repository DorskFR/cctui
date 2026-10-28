# Server core and workspace split

## Release lane and compatibility contract

Implementation targets the `0.27.0` integration branch. Feature PRs target that
branch, and each publishable increment uses `0.27.0-beta.N`. Stable promotion to
`main` happens only after the acceptance gates below pass. The existing release
workflow already tests version branches and publishes beta tags as prereleases;
`:latest` remains on the stable line. Read `docs/release-channels.md` before tagging.

`cctui-server` and the `cctui` image remain the full distribution. Existing
commands, environment defaults, API paths, client wire formats, database records,
encryption keys, mounts, enrollment and self-update selection must keep working.
The first extraction changes no migrations or deployment configuration. The new
core binary/image will be opt-in; running an existing installation never silently
selects core. Workers and dispatchers remain shared between distributions.

This is an execution/workspace boundary, not an enterprise license boundary.
GitHub review and YouTrack triage remain integrations calling the execution API.

## Ownership

| Area | Core | Workspace |
| --- | --- | --- |
| Identity | Users, service identities, API keys, scopes, revocation, ownership, audit | Interactive login adapters and identity screens |
| Accounts | Encrypted provider credentials, OAuth setup/refresh, pools, model routing | Account dashboards and historical charts |
| Gateway | Provider proxying, ephemeral tokens, limits, budget enforcement | Optional tracing exporters and reporting |
| Targets | Machine/dispatcher enrollment, daemon transport, existing dispatcher backends | Filesystem browser, interactive terminal, previews |
| Launch | JSON machine/dispatcher launch, harness/model/configuration selection | Multipart uploads, drafts, interactive context resolution |
| Policy | Dispatch grants, permission ceilings, tool/network policy, child-spawn limits | Human approval/question/plan interfaces |
| Lifecycle | Stable run identity, acknowledgment, status, cancellation, result/failure | Conversation navigation and organization |
| Events | Lifecycle, accounting, bounded diagnostic records | Full transcript archive, search, images, filesystem links |
| Inputs | Prompts/configuration references and worker context-pack execution | Server attachment storage, skill editors, shared memory |
| Operations | Health/readiness, essential metrics, configuration, migration orchestration, cleanup | Voice, plugins, rooms, federation |

Worker repository checkout, local files and result files remain execution inputs
and outputs. Removing server uploads must not remove these capabilities.
Unattended runs must not hang indefinitely waiting for a human: define a blocked
or terminal outcome and an explicit supported resolution path, preserving policy
enforcement. Do not equate removing approval screens with granting permission.

Core must be administrable through API/CLI without a workspace UI: provision a
user, issue/revoke keys, connect provider accounts, enroll targets and inspect work.
OAuth browser consent may still be required by a provider; no workspace frontend
should be required to initiate or finish the supported flow.

## Dependency direction

- `cctui-server-core`: shared identity, account, gateway, execution and essential
  persistence services. No dependency on workspace/full server.
- `cctui-core-server` (planned): small automation host using core services.
- `cctui-server-workspace` (planned): interactive services depending on core.
- `cctui-server`: existing full binary assembling core and workspace in one process.

Start with these coarse crates. Extract search/media/voice/plugin crates further
only when useful. Core must compile/test on its own without search, image codecs,
WebAuthn or UI/plugin startup. Shared protocol types can continue describing the
full API; the core binary must not need implementations of workspace capabilities.

## Increments

### Shared identity and storage foundation

The initial beta extracts `auth` and the repositories for ACLs, provider accounts,
account pools/redirects, gateway tokens, dispatch records, spawn capabilities and
the usage ledger into `cctui-server-core`. The full server re-exports the same
implementations, so callers and wire shapes stay unchanged. The core token rebind does not require attachment storage; the full server wraps
it to preserve attachment rebinding. Existing tests move
with their implementation and run under the workspace test gate. Database-gated
core tests require an already migrated database, just as the full-server tests do.

This beta is still the full server. It is not a minimal installation and does not
claim to remove workspace schema/extensions or publish a standalone core image.

### Account resolution, gateway policy and launch tracking (`0.27.0-beta.2`)

Account ranking, pool resolution and provider/path configuration now live in
`cctui-server-core`. Full-server modules re-export them, preserving route callers
and configuration defaults. `AccountResolutionContext` supplies only the pool,
the local reauthentication fallback and an asynchronous usage reader. The full
host delegates that reader to its existing gateway implementation, retaining
usage refresh/cache, history and broadcast ordering. Core never calls a workspace
route or imports `AppState`.

Account precedence, scoped pool lookup, provider-family/model filtering,
redirect-aware scoring, in-flight reservations, reauthentication exclusions and
best-effort reads retain their existing SQL and policy. The active-liveness
constant is shared with the full session display to prevent threshold drift.
The new database regression restricts search_path to temporary execution tables;
no workspace table is visible. A separate closed-database regression checks
reauthentication fallback and distinguishes database failures from rejection.

Core also owns pending-command metadata, daemon-verdict waiters and the early
launch acknowledgment deadline. The full state module re-exports these types and
functions, keeping its existing callers. Waiter registration now cleans up when
a waiting request is canceled as well as on verdict and timeout; the cancellation
regression fails against the old implementation. Existing acknowledgment tests
move with their implementation, and expiry tests check stale command pruning.

This remains an internal extraction shipped through the full distribution.
OAuth refresh, usage fetching, execution state, launch, lifecycle, standalone
hosting and minimal schema provisioning are still subsequent work.

### Provider credentials and refresh (`0.27.0-beta.3`)

Core now resolves gateway tokens to live credentials, refreshes OAuth credentials,
and persists reauthentication flags. `CredentialContext` supplies the host's
pool, HTTP client, per-account locks, reauthentication cache and lazy vault-key
accessor. The full adapter calls the existing startup-resolved vault-key helper;
installed legacy/generated keys keep working. Once loaded, static credentials
bypass database, vault-key and refresh access during token selection.

Refresh retains the second database read under the account lock, rotated-token
persistence, provider endpoints/client IDs, skew, rejection backoff and transient
failure handling. Owner/share revocation SQL is unchanged. Reauthentication
writes keep their existing cache-transition and background-write behavior.
Only the host interface allows token endpoint overrides for deterministic tests;
full-server defaults still read the same environment configuration.

Focused PostgreSQL tests cover concurrent refresh making exactly one OAuth call,
rotation of encrypted credentials, rejected-token backoff and flag persistence,
static credentials and owner/service-token revocation. Rotation and flag tests
see only the provider table, without workspace storage. Full gateway routes
re-export or forward to these shared services.

This remains the full distribution. Gateway proxying, usage fetching, target
transport, execution assembly, standalone hosting and schema provisioning remain.

### Quota enforcement and metering (`0.27.0-beta.4`)

Core now owns provider quota reads, compatible-endpoint probes, cost calculation,
billing reconciliation, session metering and the current-usage cache. The full
host re-exports those implementations and supplies its existing outbound policy
and guarded HTTP client. Probe validation remains mandatory at the host boundary.

`UsageContext` caches a fresh reading before awaiting its reporting callback. The
full callback retains rendering/broadcast followed by background history recording;
core does not depend on historical samples or the workspace event bus. Failed
upstream reads preserve the previous payload and timestamp, including the existing
fail-open behavior when no reading exists. Cache TTLs and provider defaults remain
unchanged. The release-configured provider user-agent version is now baked into
core by its own build script, preserving release behavior after the move.

Contract tests exercise fresh-cache reuse while reporting is blocked, stale-cache
fallback on HTTP 429, and host rejection of a compatible probe before network IO.
They expose only a temporary provider table; the quota and reset endpoints use a
local HTTP fixture. Existing pricing, billing and probe mapping tests move with
the implementations. This is still the full distribution; proxy transport,
execution hosting and minimal database provisioning remain subsequent work.

### Target transport and prepared machine launch (`0.27.0-beta.5`)

Core owns the delivery bus, dispatcher interface/registry and enrolled/HTTP
implementations, legacy live-session registry and policy shapes, outbound URL
policy, HTTP client setup and rolling gateway RPM/TPM windows. The full server
re-exports them and supplies the same peer transport, connection registries,
clients and shared allowlist. Full-host session addressing and daemon reconcile
assembly remain adapters above the bus. Protocol payloads are unchanged.

Parked daemon and dispatcher replies now unregister when callers are canceled,
including while waiting to send. The regression fails against the unguarded
implementation. Existing bus ownership, priority delivery, reply correlation,
dispatcher restart/hold bounds and status mapping tests move into core. HTTP
contracts cover request authentication/body, older replies, queue status and
invalid-intent versus backend errors using a local fake executor.

`launch_machine` owns prepared spawn delivery, failure metadata and the existing
early acknowledgment deadline. The full host still authorizes the target, resolves
accounts, prepares workspace context/uploads and capabilities, and rolls back
attachments when delivery fails. Core preserves the command/session key and
retains delivered command metadata for eventual daemon results. Canceling a caller
abandons its reply waiter; it does not cancel work already delivered. Focused
headless contracts cover offline cleanup, immediate refusal and delayed verdicts.

The standalone host, shared launch preparation/credential minting, lifecycle
persistence, ownership inventory and minimal provisioning are still in progress.

### Credential minting and resume (`0.27.0-beta.6`)

Core now owns account-provider binding, per-family gateway token minting,
account/model redirects, account environment and settings resolution, and
credential restoration on resume. `MintContext` supplies the pool, the installed
vault key, the external gateway URL and transient orphan-block cleanup. The full
host delegates to its existing resources; launch paths retain the same services,
SQL, token TTL and provider environment names.

The shared-account settings and environment allowlists also live in core. Owners
retain their own configuration; grantees receive only allowed keys, and a revoked
share blocks both initial mint and resume. Share revocation still cuts only the
ex-grantee's live credentials. Session token revocation, capability/budget map
cleanup and spawn-key rebinding remain full-host lifecycle adapters for now.

New PostgreSQL contracts see only temporary execution tables. They exercise
stable token reuse, orphan-block cleanup on reuse, independent provider families,
resume after revocation issuing fresh tokens without resurrecting dead ones,
gateway keys overriding account environment, and grantee settings/environment
filtering plus revocation. Existing mint, redirect and allowlist tests move with
the implementation; catalog parity tests stay in the full host. This is still the full distribution; standalone hosting and
minimal schema provisioning remain subsequent gates.

### Lifecycle persistence and audit (`0.27.0-beta.7`)

Core now persists daemon session registration, terminal state and failed-spawn
rows, and records lifecycle audit events. The same services enforce machine/user
ownership, preserve parent relationships and sticky states, repair pre-registration
account bindings, revive spurious offline ends and cap end details at UTF-8
boundaries. Session status vocabulary and basic execution queries are also shared;
full conversation projections remain in the workspace host.

`SessionEndJournal` makes transcript persistence an explicit host responsibility.
The full journal retains its ownership-guarded, deduplicated end-event insert and
must finish before the shared row update. Its errors still propagate. The headless
journal intentionally writes no transcript. Failed-spawn persistence uses the same
journal and keeps the existing insert-before-verdict behavior in the full relay.
Audit recording preserves its detached insertion, warning on failure, enrichment,
retention and persistence-before-broadcast order. Re-registration emits its revival
audit before repairing the token binding, as before.

A regression with the original transcript insert fails against execution-only
tables because `stream_events` is absent. Five headless PostgreSQL contracts cover
registration ownership and metadata, account binding repair, archive-independent
terminal/failed-spawn persistence, journal failure before mutation, spurious-offline
revival and audit persistence before broadcast. Full daemon and audit regressions
continue exercising the workspace journal and existing event ordering.

This remains an internal extraction. Lifecycle ingestion, dispatch preparation,
standalone hosting and minimal provisioning still need their remaining gates.

### Shared dispatch preparation and policy (`0.27.0-beta.8`)

Core now owns dispatcher resolution, worker identity, ephemeral credentials,
account/pool routing, model aliases, provider-family collision checks, gateway
environment injection, child-spawn ceilings, policy reservations/refunds, dispatch
handles and execution records. `DispatchContext` supplies the existing host's
execution resources and explicit callbacks for notifications, completion-webhook
registration, default capabilities and workspace context recording. Full HTTP
handlers and historical projections remain adapters; they retain their routes,
authorization and side-effect ordering. Webhook validation still uses the same
outbound policy, and workspace context ownership checks remain in the full host.

Policy normalization, request checks and the policy repository are shared too.
No-policy callers retain existing behavior; a policy-bound worker cannot grant
itself children, and a failed capability write still prevents forwarding.

The headless concurrency contract exposed a first-dispatch race: two requests
could both observe no dispatch machine, then one failed its unique-user insert
with a 500. Provisioning now converges on the installed machine/key through an
upsert. Existing keys are preserved, including concurrent requests; legacy rows
with missing keys are repaired in place.

Execution-only PostgreSQL contracts exercise authenticated HTTP forwarding,
worker credential scopes/expiry, durable handles and outcomes, reserved-env and
scope rejection before minting, host callback ordering, failure/dedup refunds,
policy child-spawn ceilings, concurrent daily caps and legacy key repair. The
full dispatch and policy regressions continue through the same shared service.

Standalone hosting, gateway proxy hosting, complete lifecycle ingestion and
minimal provisioning remain subsequent gates.

### Shared gateway pipeline (`0.27.0-beta.9`)

Core now owns the provider proxy pipeline, tool-call policy enforcement, durable
soft-limit signalling, session budgets, orphan-token protection, provider outage
cache and account failover. The full host retains its four gateway endpoints and
supplies the same installed credentials, outbound policy, clients and shared rate
windows. Path validation, credential replacement, authentication-stage errors,
quota admission, retry/failover, statistics and block clearing keep their order.

`ProxyContext` requires execution resources and explicit host decisions about
optional observers. Tool-policy enforcement always runs in core, including when
both tracing and voice are absent. A failed policy lookup without a cached policy
still refuses the request. `TraceSink` receives the captured bytes only after
core metering and TPM accounting; the full adapter retains reconstruction, cache
annotation and Langfuse delivery. `ResponseTee` retains the full voice observer
after tool enforcement. Sampling, raw request forwarding, buffering predicates,
compression handling and the bounded observer channel's `try_send` behavior are
unchanged. Usage notices remain an awaited host callback before clearing the
credential rejection flag; shared notice delivery is a standalone-host gate.

Execution-only PostgreSQL contracts exercise byte preservation, upstream
credential replacement, status/headers, RPM/TPM admission, outbound/path denial,
separate orphan/provider authentication errors, missing-policy refusal and tool
blocking with no workspace observers. The tool-block contract waits for the
post-persistence event before checking its audit row. A sampled tracing contract
checks metering before reporting and unchanged response bytes. Existing gateway,
policy rewriting, failover and provider-status regressions move with their code;
the full host continues testing its workspace voice and usage-notice adapters.

This remains the full distribution. Standalone hosting, shared machine spawn
preparation, complete lifecycle ingestion, workspace assembly and minimal schema
provisioning remain subsequent gates.

### Execution services and complete ownership inventory

- Inventory every route, background task, setting, event handler and table,
  including internal/daemon/gateway routes outside the user API registry.
- Extract `CoreState`, account resolution/refresh, gateway services, machine spawn
  and dispatcher execution from full `AppState` and multipart/context handling.
- Preserve grant/ceiling intersection, ownership, temporary credential rebind and
  revocation, dispatcher reservations, failure visibility and cancellation.
- Define acknowledgment, run/session ID correlation, result retention and
  diagnostic bounds for both machine and dispatcher execution.
- Separate lifecycle handling from transcript persistence/indexing. Keep replay
  deduplication and daemon transcript acknowledgments correct; metadata needed
  for protocol acknowledgments is not a searchable conversation archive.
- Introduce explicit interfaces for workspace input preparation, payload storage,
  indexing and session cleanup. Specify ordering, failure and backpressure rather
  than introducing fire-and-forget hooks that change full-server behavior.

Gate: full-server route/auth contract and lifecycle behavior remain unchanged;
core execution services have no workspace dependency.

### Opt-in automation host

- Add `cctui-core-server`, JSON launch and headless administration.
- Extend capabilities additively to distinguish supported features from enabled
  integrations; older servers remain compatible with existing clients.
- Keep full routes registered by default. Clients connected to core explicitly
  report unavailable workspace functions. Update protocol/TUI parity together.
- Essential workers accept existing worker contract v1. Avoid a separate worker
  image rewrite in this project.

Gate: one service user can connect accounts, dispatch to a machine or Kubernetes
worker, observe completion/failure, retrieve a result and cancel work without
voice, uploads, search, plugins or workspace services. Exercise offline targets,
expired credentials, crashes, duplicate submissions and callback failure. State
actual execution/callback guarantees; durable retries or exactly-once execution
are separate behavior changes, not implicit properties of extraction.

### Database provisioning

Keep historical migration files/checksums immutable during extraction. Before
core is production-ready, implement three tested provisioning paths:

1. Existing full database: retain the historical ledger and serialized upgrade
   path, then establish package-owned forward migration tracking additively.
2. Empty core database: install only core schema, without workspace tables or
   `pg_trgm`/search extensions.
3. Core database to full: install workspace schema/extensions without recreating
   users, accounts, keys or execution data.

Both provisioning paths must converge on compatible core constraints and tables.
Use explicit schema lineage/version checks and one migration coordination lock.
Do not fake applied historical records or rewrite migration SQL to skip features.
Do not delete data or extensions merely because core runs against a legacy full
schema. Audit foreign keys, triggers, views, settings defaults, seed data and
queries before declaring a table optional.

Define and test a supported rollback window. Tolerating newer migration ledger
entries does not prove old binaries understand a changed schema. Starting a
pre-split binary against a fresh core database is unsupported until proven.

### Packaging, beta validation and promotion

- Add opt-in core artifacts/image and include them in release scope, build/cache,
  signing, binary verification, deployment examples and release assets.
- Preserve full image identity and self-update selection. No automatic switch to
  core, including when following beta updates.
- Use production-shaped upgrade fixtures covering keys, vault data, accounts,
  machines, dispatchers, transcripts, images, uploads and settings.
- Test rolling old/new full servers and supported old workers/clients. Test fresh
  core, core-to-full expansion and supported rollback against the same database.
- Review TUI parity changes and run `cargo test -p cctui-tui` before every tag.
- Publish beta tags from green commits on `0.27.0`; never merge beta development
  into `main` just to trigger CI. Deploy only to the opted-in beta environment,
  verifying the served version, account access and real worker lifecycle.
- Promote the final release through a PR into `main`, stable version bump, green
  CI, signed tag and verified rollout. Keep the stable release lane independent
  while these changes are incomplete.

## Estimate and exclusions

Initial planning estimate: 6–10 engineer-weeks including database provisioning,
packaging and compatibility validation. Re-estimate after the execution inventory;
shared SQL dependencies and lifecycle extraction are the largest uncertainties.
The initial storage extraction does not satisfy the standalone-core gates.

Organization hierarchies, enterprise SSO, new workflow engines, new scheduling
semantics, durable callback delivery and worker-image slimming are separate work.
