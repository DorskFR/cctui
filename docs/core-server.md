# Standalone execution server (experimental)

The opt-in `cctui-core-server` binary and `ghcr.io/dorskfr/cctui-core` image
provide identity, encrypted provider accounts, credential proxying, machine and
dispatcher enrollment, JSON dispatch/spawn, run results and cancellation. They do
not serve a workspace UI, transcript search, uploads, plugins, voice or previews.
Existing `cctui-server` installations and the `cctui` image remain the full product.

This is an experimental beta. Verified runs include real Claude/Codex machine
execution, a Codex parent calling a Claude child and a sandboxed Kubernetes Codex
Job with a fetched context pack. Same-schema core restart/rollback and mixed-version
full-server rollout/rollback have also passed controlled-provider acceptance.
Machine-authenticated child spawning, follow-ups,
completion delivery, archival and session-limit reporting are available with
capability and budget enforcement; workspace agent profiles require full.
Use the execution API for this beta. Current webui and TUI clients identify core
and explain which workspace functions require a full server. Server self-update
is disabled.
Daemon manifests/downloads and machine connectivity verification use the same
contracts as full. Follow the
[split roadmap](server-core-split.md) for production-readiness gates.

## Install

Release assets include `cctui-core-server-linux-amd64`,
`cctui-core-server-linux-arm64` and `cctui-core-server-darwin-arm64`, with the
same signed checksum files as the other binaries. The image is Linux amd64.
Select an explicit beta version; the stable full deployment need not change.

The server requires PostgreSQL and these environment variables:

| Variable | Purpose |
| --- | --- |
| `DATABASE_URL` | PostgreSQL connection URL; a fresh core database needs no optional extensions |
| `CCTUI_VAULT_KEY` | Hex-encoded encryption key of at least 32 bytes; retain it across upgrades |
| `CCTUI_ADMIN_TOKENS` | Comma-separated bootstrap administrator bearer tokens |
| `CCTUI_EXTERNAL_URL` | Server URL reachable by workers, used for gateway routing |
| `CCTUI_HOST`, `CCTUI_PORT` | Listen address; defaults `0.0.0.0:8700` |

Generate and store the vault key and bootstrap token before starting. For example,
`openssl rand -hex 32` generates each value. Put configuration in a protected env
file and supply it to the process or container. For a local PostgreSQL test:

```sh
docker run --rm --env-file /path/to/core.env --network host \
  ghcr.io/dorskfr/cctui-core:0.27.0-beta.34
```

`DATABASE_URL` and `CCTUI_EXTERNAL_URL` must be reachable from the container's
network. Put the API behind your usual TLS endpoint for remote workers.
There are no workspace file-volume requirements. Persist PostgreSQL and the
vault key. The image runs as UID 10001.

Normal startup provisions or validates the database. To provision without
starting HTTP, use `cctui-core-server --init-db` (only `DATABASE_URL` is required),
or append `--init-db` to the image invocation. `--version` needs no configuration.
`GET /health` checks HTTP liveness; `GET /ready` also checks the database.

## Headless setup and dispatch

Set `CCTUI_URL` and `CCTUI_ADMIN_TOKEN` in your shell to the server URL and one
bootstrap token. Create a service user:

```sh
curl --fail-with-body "$CCTUI_URL/api/v1/admin/users" \
  -H "Authorization: Bearer $CCTUI_ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"automation"}'
```

Store the returned `key` securely as `CCTUI_USER_KEY`; it is shown once. It grants
read, enrollment and dispatch access. Use `POST /api/v1/users/{id}/keys` with an
administrator token to issue narrower keys; `DELETE /api/v1/users/{id}/keys/{kid}`
revokes them. A read-only key uses `{"label":"observer","scopes":["read"]}`.

Create provider credentials through `/api/v1/accounts`, or initiate provider
consent through `/api/v1/accounts/oauth/start` and finish through
`/api/v1/accounts/oauth/finish`. These API shapes match the full distribution.
Account credentials are encrypted with the vault key and redacted in responses.

Enroll a machine with `POST /api/v1/enroll` or a dispatcher with
`POST /api/v1/dispatcher/enroll`, authenticated as the service user. Enrollment
returns the worker credential. Configure the matching daemon/dispatcher with it.
The HTTP dispatcher adapter can instead be configured with
`CCTUI_HTTP_DISPATCHERS`, a JSON array of `{ "id", "url", "token" }` objects
(`token` is optional). Its executor must implement the existing dispatch contract;
a generic webhook does not execute a workload.

Starting with beta.33, inspect a machine's enabled harnesses and reported binaries
with `GET /api/v1/machines/{id}/adapters`. Read access requires the machine's owner,
an administrator or an active machine share, plus read scope. Configure a harness
with the bootstrap administrator token; changes reconcile the connected daemon
without reenrollment and leave running sessions alone:

```sh
curl --fail-with-body -X PUT "$CCTUI_URL/api/v1/machines/$MACHINE_ID/adapters/claude-code" \
  -H "Authorization: Bearer $CCTUI_ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"enabled":true,"config":{"mode":"oneshot"}}'
```

`DELETE /api/v1/machines/{id}/adapters/{adapter}` with an administrator token
removes the machine's pin and restores the table default. These are the same
configuration and access contracts as the full distribution; selecting a mode
does not change a session's permission policy.

For a configured HTTP executor named `automation`, a dispatch request is:

```sh
curl --fail-with-body "$CCTUI_URL/api/v1/sessions/dispatch" \
  -H "Authorization: Bearer $CCTUI_USER_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"dispatcher":"automation","payload":{"prompt_file":"/work/task.md"}}'
```

The prompt file must exist where the executor expects it. Keep the returned
`session_id`. Fetch `/api/v1/sessions/{session_id}/result` with the same user key
for status/outcome, or POST `/api/v1/sessions/{session_id}/kill` to cancel.
An executor acknowledgment means dispatch was accepted; it does not establish
that the task completed. Failure notifications use the existing bounded retry
policy and may be duplicated around crashes; they are not exactly-once delivery.

Starting with beta.34, machine cancellation also accepts the returned command ID
before registration and after reconnect/restart. The terminal result is `killed`; any captured partial assistant answer is retained.
The host commits the stop decision, gateway-token and temporary worker-key
revocation, capability cleanup and release of unregistered child reservations
before acknowledging cancellation. Registered child grants remain charged to the
tree budget. Late registration/verdict/end events cannot revive stopped work, and
credential pulls or child follow-ups return 410 for a stopped execution.
Worker stop delivery is best effort when its machine is offline; a later
announcement is stopped again. This does not establish exactly-once execution.

For a queued dispatcher run, cancellation calls the backend's existing cancel
operation before recording success. An unavailable backend returns an error;
an adapter without cancellation support returns 501 and keeps the run pending.
A request whose backend handle has not been recorded returns 409 until it can
be cancelled or its registration deadline settles it. The configured HTTP
dispatcher currently has no cancellation operation.

## Database transitions

- **Empty → core:** creates execution tables and package lineage, without
  transcript-search extensions or a historical full migration ledger.
- **Existing full → core:** validates and adopts the supported full schema;
  workspace tables and stored data remain intact.
- **Core → full:** start the matching full beta with the same database and vault
  key. Workspace schema/extensions are added and execution data is preserved.

Existing full databases keep their historical migration records and checksums.
Do not point a pre-split full binary at a database originally bootstrapped by core;
that rollback is unsupported. Keep the normal database backup before upgrades.
Beta 25 adds package schema version two for durable machine launches. Both core and full betas
22–24 reject this version; rolling back across that boundary requires a
pre-upgrade database restore. Machine result polling accepts the returned command
ID and follows canonical worker registration after a host restart.
Beta 34 adds package schema version three for durable stop decisions. Core/full
betas 25–33 reject this version; rollback requires a pre-upgrade database restore.
Unregistered machine launches expire after ten minutes. Dispatcher requests with
unknown delivery, or forwarded workers that never register, expire after fifteen
minutes; time held in the dispatcher queue does not count. An independent task
reconciles deadlines every thirty seconds in batches of up to 100 per target.
Unknown delivery becomes `spawn_failed`; work is never automatically resent.

Switching full to core deliberately removes access to workspace features and is
an operator choice, separate from an ordinary full-server upgrade.


## Repeatable machine execution smoke

On Linux with the repository's PostgreSQL test container running on port 5481:

```sh
cargo build -p cctui-core-server
python3 scripts/check-core-worker.py
```

This starts an isolated core database, enrolls a containerized real daemon,
launches the actual Claude harness and verifies the final result through a
controlled provider endpoint and core's credential gateway. It makes no paid
provider requests. It removes its own database/container/work directory and
prints the directory retaining diagnostics. Ports 18727 and 18729 must be free.
`CCTUI_TEST_WORKER_IMAGE` selects a worker release for compatibility checks;
`CCTUI_TEST_POSTGRES_CONTAINER` selects the test PostgreSQL container.
The smoke configures oneshot mode through the administrative harness API.
Only compatibility runs starting an older core without that API seed the old
fixture database directly. Use the upgrade and ownership modes below for
host-transition acceptance.

The same fixture supports `--adapter codex`. For real MCP discovery, parent/child
execution, registered-child slot retention and archival:

```sh
cargo build -p cctui-core-server -p cctui-daemon
CCTUI_TEST_WORKER_IMAGE=ghcr.io/dorskfr/cctui-worker:0.27.0-beta.26 \
CCTUI_TEST_DAEMON_BINARY="$PWD/target/debug/cctui-daemon" \
  python3 scripts/check-core-worker.py --child
```

The optional daemon override mounts the newly built Linux daemon in the published
worker image, retaining its real harness binaries. The Codex fixture selects full
access explicitly inside the disposable container. Claude children use `auto`.
The controlled provider returns a real `CctuiAgent` tool call, validates the child's
result in the parent's next request, then checks the completed child still occupies
one slot and archiving it releases that slot.

## Client compatibility

The capability endpoint identifies core with `distribution: "core"` and
`workspace: false`. Current TUI and web clients explain that interactive sessions
and workspace screens require a full server and direct core users to the execution
API. Older full-server capability responses continue to select the usual client
flow. Execution API support and configured optional integrations are separate
capabilities.


## Repeatable Kubernetes execution smoke

The same fixture can enroll a real Kubernetes dispatcher and launch a sandboxed
Job through an existing disposable cluster's WorkerProfile and admission webhooks
(see [worker-profile-crd.md](worker-profile-crd.md)). Pass the kubeconfig explicitly;
the fixture never selects a default context. The profile must permit the test host
as private egress, and its worker image must contain the boot-dispatch lifecycle
fix from beta.30 or later. Keep both guard ConfigMaps (`guard-proxy-env` and
`guard-proxy-inject`) present, even when empty.

```sh
python3 scripts/check-core-worker.py --adapter codex \
  --kubeconfig /tmp/core-acceptance-kubeconfig \
  --worker-url http://HOST_REACHABLE_FROM_PODS:18727 \
  --namespace core-acceptance --profile core-smoke
```

The fixture uses a published dispatcher (`CCTUI_TEST_DISPATCHER_IMAGE` overrides
it), a fresh core database and the controlled provider. It verifies the terminal
API result and final answer, retains Job/container diagnostics and deletes only
its own Job and dispatcher container. The operator owns cluster/webhook cleanup.
The Kubernetes smoke passed with the published beta.30 worker for Claude and
Codex, including signed pinned context packs. This is
single-server acceptance; it does not establish rolling or multi-replica support.


Add `--context-pack` to the Kubernetes command to serve a temporary git pack,
fetch its exact signed revision inside the worker, and verify both `AGENTS.md`
instructions and the named prompt reach the real harness's provider request.
This mode requires the checkout's configured Git signing key. The fixture serves
only that temporary pack and removes it afterward. This acceptance passed for
Claude and Codex; other adapter targets retain their existing staging tests.


## Same-schema core upgrade and rollback smoke

To test persistent identities, encrypted provider accounts, completed results and
live-worker reconnection across a binary change, pass an older downloaded core
binary to the machine fixture:

```sh
CCTUI_TEST_CORE_BINARY="$PWD/target/debug/cctui-core-server" \
  python3 scripts/check-core-worker.py \
  --upgrade-from /tmp/older-core/cctui-core-server-linux-amd64
```

It starts the older binary, completes a real Claude task, starts the current
binary against the same database, then starts the older binary again. After each
transition the existing key, provider account and prior result must remain
readable, and the same daemon must reconnect and complete a new task through the
credential gateway. No database restore or worker reenrollment is performed.
This checks sequential core restarts within a compatible schema window. It does
not establish rolling multi-replica behavior or rollback across incompatible
package schemas; the beta.22–24 restriction above still applies.


## Standalone host ownership

Starting with beta.32, run **one active standalone core per database**. Its daemon
connections and dispatch transport are local to the process. A database advisory
lock refuses a second standalone host before schema provisioning or listening.
The lock is held on a dedicated physical connection; losing that connection or
exceeding the bounded health probe exits the host. Administrative `--init-db`
remains available while the host runs.

Use a stop-then-start deployment strategy for core, retain the database and vault
key, and let workers reconnect. Configure restart supervision as usual. Do not
mix core and full hosts against one database at the same time: the standalone
lock coordinates standalone hosts only. Older core betas do not take this lock.
The full distribution retains its existing multi-replica peer transport.

To verify duplicate refusal, lease-loss process exit, replacement startup and a
fresh real task on the same reconnected daemon:

```sh
python3 scripts/check-core-worker.py --check-ownership
```

## Full-server rollout and rollback acceptance

The same fixture supports the full distribution, retaining a real transcript,
encrypted provider credentials, user settings, image bytes, an uploaded file in
a shared blob mount and dispatcher enrollment/authentication. The local TCP
proxy lets existing daemon connections survive an HTTP replica switch. Each
mixed-version phase must route a new task through the peer holding the daemon,
then reconnect that same daemon after the previous owner exits. Rollback repeats
both directions without restoring the database or reenrolling the worker.

```sh
cargo build -p cctui-server
CCTUI_TEST_SERVER_BINARY="$PWD/target/debug/cctui-server" \
  python3 scripts/check-core-worker.py --full-server --rolling \
  --upgrade-from /tmp/older-full/cctui-server
```

Use `--full-server --upgrade-from ...` without `--rolling` for sequential full
restarts. `CCTUI_TEST_SERVER_BINARY` selects either distribution; the existing
`CCTUI_TEST_CORE_BINARY` override remains supported. Use a published older binary
and a compatible schema window. The mixed full acceptance has passed against
stable 0.26.1 on a legacy-origin database; it does not change the core-origin
rollback restriction above. Full fixture modes currently use the Claude machine
target. They scrub inherited operator configuration and retain diagnostics while
removing their own processes, worker container and database.

## Repeatable cancellation acceptance

```sh
CCTUI_TEST_WORKER_IMAGE=ghcr.io/dorskfr/cctui-worker:0.27.0-beta.33 \
  python3 scripts/check-core-worker.py --check-cancellation
```

The controlled provider holds a real harness request while the fixture cancels
through its command ID. It checks the terminal outcome, credential revocation,
refused reminting and a fresh task on the same daemon. This mode can be combined
with `--check-ownership` to exercise cancellation after lease-loss recovery.
