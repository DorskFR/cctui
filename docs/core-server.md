# Standalone execution server (experimental)

The opt-in `cctui-core-server` binary and `ghcr.io/dorskfr/cctui-core` image
provide identity, encrypted provider accounts, credential proxying, machine and
dispatcher enrollment, JSON dispatch/spawn, run results and cancellation. They do
not serve a workspace UI, transcript search, uploads, plugins, voice or previews.
Existing `cctui-server` installations and the `cctui` image remain the full product.

This is an experimental beta. Verified runs include real Claude/Codex machine
execution, a Codex parent calling a Claude child and a sandboxed Kubernetes Codex
Job with a fetched context pack. Upgrade acceptance remains open.
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
  ghcr.io/dorskfr/cctui-core:0.27.0-beta.30
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
ID and follows canonical worker registration after a host restart. An interrupted
delivery can remain pending; the server does not automatically resend the work.

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
The smoke pins oneshot mode directly in its temporary fixture database; general
headless harness configuration remains a separate API gap. This is a machine
execution check; rolling server upgrades require a separate gate.

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
The Kubernetes smoke passed with the beta.30 daemon layered over the published
beta.26 worker, retaining that image's harnesses and isolation machinery. This is
single-server acceptance; it does not establish rolling or multi-replica support.


Add `--context-pack` to the Kubernetes command to serve a temporary git pack,
fetch its exact signed revision inside the worker, and verify both `AGENTS.md`
instructions and the named prompt reach the real harness's provider request.
This mode requires the checkout's configured Git signing key. The fixture serves
only that temporary pack and removes it afterward. This acceptance passed for
Codex; other adapter targets retain their existing staging tests.
