# Core

Core is the cctui server started with the core feature file,
[`deploy/features/core.toml`](../deploy/features/core.toml), which turns every
switchable feature off. What is left runs dispatched workers: users and keys,
accounts and their OAuth credentials, model aliases, usage and limit resets,
enrolled dispatchers, dispatch, the gateway the workers' model traffic goes
through, and reading the sessions that result. There are no enrolled
machines, no spawning on them, no self-update; workers run the binary their
image ships. [Features](./features.md) lists what each switchable feature
carries.

It is the same `cctui` server image as a full deployment. Only the feature
file differs.

## Deploying

[`deploy/core`](../deploy/core) is a kustomize example:

```sh
kustomize build deploy/core
```

renders

| Kind | Name | What |
|---|---|---|
| ConfigMap | `cctui-core-features-<hash>` | `core.toml`, mounted at `/etc/cctui/features/core.toml` |
| Deployment | `cctui-core` | The server, `ghcr.io/dorskfr/cctui`, one replica, port 8700 |
| Service | `cctui-core` | Port 8700 |
| Deployment | `cctui-core-ui` | The web UI, `ghcr.io/dorskfr/cctui-ui`, port 8080 |
| Service | `cctui-core-ui` | Port 8080 |

It creates no database, no volume and no Secret. Provide them:

1. A PostgreSQL database (see [Postgres](#postgres)).
2. A Secret named `cctui-core` in the namespace you deploy to:

   ```sh
   kubectl create secret generic cctui-core \
     --from-literal=database-url='postgres://cctui:PASSWORD@HOST:5432/cctui' \
     --from-literal=vault-key="$(openssl rand -hex 32)" \
     --from-literal=admin-tokens="$(openssl rand -hex 32)"
   ```

   `vault-key` encrypts the provider credentials stored in the database.
   Keep a copy: without it they cannot be read back. `admin-tokens` is the
   bootstrap admin token (comma-separated for several).
3. An overlay that sets your public URL and puts an ingress in front, with
   both services on one host: `/api/` and `/gateway/` to `cctui-core:8700`,
   everything else to `cctui-core-ui:8080`. The UI only calls its own
   origin.

   ```yaml
   # kustomization.yaml of your overlay
   resources:
     - github.com/DorskFR/cctui//deploy/core?ref=v0.30.0
     - ingress.yaml
   patches:
     - target: { kind: Deployment, name: cctui-core }
       patch: |
         - op: replace
           path: /spec/template/spec/containers/0/env/3/value
           value: https://cctui.example.org
   ```

   `env/3` is `CCTUI_EXTERNAL_URL`. Browsers, dispatched workers and the
   gateway addresses handed to workers all use it, so it must be the address
   workers reach the server at too.

The server environment in the example:

| Variable | Value |
|---|---|
| `CCTUI_FEATURES_FILE` | `/etc/cctui/features/core.toml` |
| `CCTUI_HOST`, `CCTUI_PORT` | `0.0.0.0`, `8700` |
| `CCTUI_EXTERNAL_URL` | The public origin. Replace `https://cctui.example.com`. |
| `CCTUI_TRUSTED_PROXY_HOPS` | `1`: the number of proxies in front of the pod. Rate limits key on the client address this recovers; set it to your real hop count. |
| `DATABASE_URL` | Secret `cctui-core`, key `database-url` |
| `CCTUI_VAULT_KEY` | Secret `cctui-core`, key `vault-key` |
| `CCTUI_ADMIN_TOKENS` | Secret `cctui-core`, key `admin-tokens` |

`DATABASE_URL`, `CCTUI_VAULT_KEY` and `CCTUI_ADMIN_TOKENS` may hold a
[secret ref](./secret-refs.md) instead of the value.

Leave these unset:

| Variable | Why |
|---|---|
| `CCTUI_POD_IP` | It joins the replica mesh. The example runs one replica, whose command bus stays in process. |
| `CCTUI_PLUGINS_DIR`, `CCTUI_PREVIEW_HOST`, `CCTUI_CCTUIVERSE`, `CCTUI_LANGFUSE_*` | Their features are off, and an off feature wins over its variable. |
| `CCTUI_EMOJI_ENDPOINT`, `CCTUI_EMOJI_MODEL`, `CCTUI_EMOJI_TOKEN` | The session emoji picker sends each session title, which is derived from the prompt, to that external model. It belongs to `organising`, which is off, so the server ignores the endpoint and logs it once at boot. |
| `CCTUI_NTFY_URL`, `CCTUI_NTFY_TOKEN` | Together they post a notification to that ntfy topic on every dispatch received or failed (dispatcher, user and session id) and on a crashlooping daemon. Nothing switches them off but leaving them unset. |
| `CCTUI_METRICS_PUBLIC` | `1`, `true`, `on` or `yes` serves `/metrics` without a token. Unset, a scrape authenticates like an API call. |
| `CCTUI_PROVIDER_STATUS` | Unset, the server polls the providers' public status pages for `GET /provider-status`. Set it to `0` to keep a core server from making those calls; every provider then reads `unknown`. |
| `CCTUI_DISPATCHERS`, `CCTUI_HTTP_DISPATCHERS` | Enroll dispatchers instead (below). |
| `CCTUI_CLAUDE_LITELLM_*` | A retired compatibility path. Add an account with a compatible provider instead. |
| `CCTUI_SPAWN_MAX_CHILDREN`, `CCTUI_SPAWN_MAX_DEPTH`, `CCTUI_SPAWN_MAX_TREE_BUDGET_USD` | Child sessions are off. |
| `CCTUI_BLOB_PATH` | Unset keeps transcript images in Postgres. Set it only to a directory every replica shares durably. |

The server is a single replica with the `Recreate` strategy: two pods would
each hold their own command bus, and a command sent through one would not
reach a worker connected to the other.

### Checking what runs

The server logs the resolved feature set once at boot:

```
features: on=[] off=[machine_spawn,files,uploads,...]
```

`GET /readyz` answers `200` with the same set on its second line, or `503`
with `database unavailable` on the first:

```
ok
features: on=[] off=[machine_spawn,files,uploads,...]
```

A route of a feature that is off answers `404` (`405` when its path also
matches a kept route of another method).

`/health`, `/readyz`, `/llms.txt` and `/api/v1/openapi.json` answer without a
token: `/readyz` prints the feature set, the other two describe the API's
shape and never its data. `PUT /admin/instance/upstream-hosts` (admin only)
widens the hosts a compatible provider may point at; leave the list empty
unless an account uses a custom upstream. A stored provider API key
(`/keys`) is write-only: no route returns its value.

## Postgres

A fresh database is provisioned at the first start; there is no separate
migration step. With core's feature set the server needs the `pgcrypto`
extension; `pg_trgm` and `btree_gin` only once `transcript_search` is on. See
[Postgres requirements](./features.md#postgres-requirements). The schema uses
PL/pgSQL.

## The first session, step by step

### 1. First login

Open the public URL. The login screen asks for a **Token**. Paste the
bootstrap admin token and press **Sign in**. The server answers with an
`HttpOnly` cookie that authenticates the browser from then on.

### 2. Create a real user, and log in as that user

The bootstrap token is the instance's break-glass admin. It belongs to no
real account holder: it has no accounts, and a dispatch made with it carries
no worker credential and no gateway access, so its worker cannot connect or
reach a model. Use it only to create users.

1. **Settings**, **Users & keys**, **New user**. Enter a **User name**. The
   user's key is shown once: copy it.
2. Optionally, open the user, the **Keys** tab, **New key**, to mint a key
   with chosen scopes, for example one for an API caller that only needs
   `dispatch`.
3. Open the account menu, **Log out**, and sign in again with that key.

Everything below is done as that user.

### 3. Connect the OAuth account

**Accounts**, **New account**. Enter a **Name**, pick the **Provider**, then
**Sign in with Claude**: a
claude.ai tab opens; authorize, copy the code it shows and paste it into
**Code from claude.ai**. For OpenAI, **Sign in with ChatGPT** opens ChatGPT;
copy the `http://localhost:1455/...` URL the tab fails to load and paste it.

### 4. Set a model alias

Open the account. Under **Provider credentials**, the gear on the credential
opens its settings under **Provider settings**. Choose **Aliases**, then
**Add alias**: a logical name and the model code it launches,
for example `fable` → `claude-fable-5-2[1m]`.

An alias applies at launch only. A dispatch whose `model` is `fable` starts
its worker on `claude-fable-5-2[1m]`; changing the alias later does not change
sessions already running.

### 5. Enroll the dispatcher

**Settings**, **Dispatchers**, **Enroll dispatcher**. Give it a **Name** and a
**Type**. **Default account or pool (optional)** is the account a dispatch
naming none routes through. Press **Enroll**. The enrollment key is shown
once.

The key goes in the dispatcher's configuration file, `dispatcher.toml`
(`$XDG_CONFIG_HOME/cctui/dispatcher.toml` by default), as `dispatcher_key`.
The value may be a [secret ref](./secret-refs.md). `--config` or
`CCTUI_DISPATCHER_CONFIG` points the dispatcher at another path. For
Kubernetes, store that file as a Secret mounted into `cctui-dispatcher-kube`; the rest of its setup
(worker profiles, admission webhooks, network policy) is in
[core on Kubernetes](./core-kubernetes.md). A connected dispatcher shows as
connected in the same list.

### 6. Dispatch a session, read it, stop it

**Sessions**, **New session**. On a core server this opens the dispatch form:

| Field | What it sets |
|---|---|
| **Dispatcher** | The enrolled dispatcher to run on (listed once one is enrolled). |
| **Harness** | The agent the worker runs, Claude or Codex. |
| **Name (optional)** | The session's label. |
| **Identity (optional)** | Which credential set the worker acts as; empty is the worker's default. |
| **Repo** | A repository checked out under the worker's `/workspace` (optional). |
| **Ticket (optional)** | An issue id handed to the flow's prompt. |
| **Prompt** | The task. |
| **Prompt file (optional)** | A file under the worker's `/prompts`; it overrides **Prompt**. |
| **Account (optional)** | The account whose gateway the worker's model traffic goes through. **Default (no account)** leaves it to the dispatcher's default account. |
| **Model**, **Timeout min** | The model to launch and the run's time limit. |
| **Context pack (optional)** | **Context pack URL**, and under **Advanced** its ref, subdirectory and token. |

Press **Dispatch**. The session appears under **Sessions** once its worker
connects; open it to read the conversation live. **Archive**, in the session's
header (under **More actions** on a narrow screen), stops it: the session's
gateway credential is revoked and its worker is told to remove its job.

The same dispatch through the API, with the user's key or any of its keys
holding the `dispatch` scope:

```sh
curl -sS https://cctui.example.org/api/v1/sessions/dispatch \
  -H "Authorization: Bearer $CCTUI_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"dispatcher": "kube", "account": "claude", "payload": {"model": "fable", "prompt_file": "review"}}'
```

The answer carries the `session_id`. The payload is handed to the worker as
is; [the worker contract](./worker-contract.md) lists the fields it reads.
`GET /api/v1/sessions/{id}/conversation` returns the transcript,
`GET /api/v1/sessions/{id}/result` the outcome, and
`POST /api/v1/sessions/{id}/kill` stops the worker without archiving the
session.

### 7. Limits and resets

Usage against each window shows on the account's credentials under
**Provider credentials**. Under **Provider settings**, **Limits** caps
cctui's own share of each window and the gateway rate; **Limit resets** lists
the resets the provider currently offers, each with a **Reset** button. The
credential's card under **Provider credentials** also offers **Reset limit**
while a reset is available.

### 8. Log out

The account menu, **Log out**. It expires the cookie. The token itself stays
valid; revoke it under **Settings**, **Users & keys** when it should stop
working.

## Turning one feature back on

Each feature is a key in the file. To keep core and add, say, rooms:

```toml
default = false

[features]
rooms = true
```

A feature whose requirement is off refuses to start the server when set to
`true`: `files = true` also needs `machine_spawn = true`. The table in
[features](./features.md) gives each feature's requirement.

In an overlay, replace the generated ConfigMap with your own file:

```yaml
configMapGenerator:
  - name: cctui-core-features
    behavior: replace
    files:
      - core.toml
```

The ConfigMap name carries a hash of its content, so the Deployment restarts
on the change. Check the second line of `/readyz` afterwards.

## Upgrading

1. Back up the database. Schema changes only move forward: going back to an
   older release means restoring that backup. A database created with
   `transcript_search` off cannot be opened by a release older than 0.30.0
   at all.
2. Change the image tags in your overlay (`images:`, `newTag`) for both
   `ghcr.io/dorskfr/cctui` and `ghcr.io/dorskfr/cctui-ui`, to the same
   release.
3. Apply. The `Recreate` strategy stops the old pod before the new one starts;
   the new one applies schema changes at boot before it reports ready.
   Connected dispatchers and workers reconnect on their own.
4. Check `/readyz`: `ok`, and the feature set you expect. A new release may
   add switchable features; under `default = false` they start off.
