# gh-review

Versioned HTTP + SSE contract for the cctui GitHub review center (epic CCT-600).

It is a Bun + TypeScript service built on [Hono](https://hono.dev) +
[`@hono/zod-openapi`](https://github.com/honojs/middleware/tree/main/packages/zod-openapi):
the `/v1` routes and their zod schemas are the source of truth, and the OpenAPI
document + TypeScript client are generated from them.

The contract surface was frozen in CCT-604. CCT-601 adds the **sync daemon**: an
ETag polling loop over octokit, a Postgres JSONB document store, and SSE push via
`LISTEN/NOTIFY`. The read routes now serve real envelopes from the store; when
`DATABASE_URL` is unset the service still boots and serves the empty contract.

## Design

- **`/v1` from day one.** Every route is under `/v1`.
- **GitHub payloads relayed verbatim.** There are zero hand-written GitHub types.
  Every record is an envelope:

  ```jsonc
  { "account": "DorskFR", "kind": "pull_request", "synced_at": "…", "etag": "…",
    "payload": { /* GitHub-shaped JSONB, unknown at the envelope level */ } }
  ```

  The frontend narrows `payload` with octokit types; the server never models it.
- **Error model** is uniform: `{ "error": { "code", "message", "details"? } }`.
- **Pagination** is cursor based: list endpoints accept `?account&limit&cursor`
  and return `{ "items": [...], "next_cursor": string | null }`.

## Routes

| Method | Path | Purpose |
| ------ | ---- | ------- |
| GET | `/v1/health` | Liveness probe (`{ ok: true }`). |
| GET | `/v1/status` | Service + sync status. |
| GET | `/v1/accounts` | List the caller's GitHub accounts (no secrets). |
| POST | `/v1/accounts` | Add an account: validate the PAT against `/user`, seal it, store it. |
| DELETE | `/v1/accounts/{id}` | Remove one of the caller's accounts. |
| GET | `/v1/repos` | List synced repositories (page of envelopes). |
| GET | `/v1/repos/{owner}/{repo}` | One repository envelope. |
| GET | `/v1/repos/{owner}/{repo}/pulls` | List synced PRs for a repo. |
| GET | `/v1/repos/{owner}/{repo}/pulls/{number}` | One PR envelope. |
| GET | `/v1/notifications` | Notifications inbox feed — envelopes joined with local state, with filters (see below). |
| POST | `/v1/notifications/state` | Bulk mark threads read/done/archived. |
| POST | `/v1/notifications/{id}/state` | Mark one thread read/done/archived. |
| GET | `/v1/events` | Server-Sent Events stream (see below). |
| GET | `/v1/openapi.json` | The generated OpenAPI document. |

## SSE event catalogue — `GET /v1/events`

`text/event-stream`. Each message has a named `event:` and a JSON `data:` payload.
The union of documented events is the `SseEvent` schema in the OpenAPI components.
The server also emits periodic `event: ping` keep-alives (empty data) that clients
ignore.

| `event` | When | `data` payload |
| ------- | ---- | -------------- |
| `pr.updated` | A synced PR's payload changed (new commit, review, comment, CI, merge state). | `{ account, owner, repo, number }` — a hint to refetch `/v1/repos/{owner}/{repo}/pulls/{number}`. |
| `notification.new` | A new GitHub notification landed for an account. | `{ account, id }` |
| `notification.updated` | A notification's local state changed (read/done/archived). | `{ account, id }` |
| `sync.status` | The sync daemon changed state (idle → syncing → error). | `{ account, state: "idle" \| "syncing" \| "error", last_run }` |

Events are **change hints**, not the data itself — clients refetch the relevant
envelope over HTTP. This keeps the SSE stream small and the HTTP cache warm.
They are wired to Postgres `LISTEN/NOTIFY`: a document upsert that changes the
payload fires `NOTIFY ghreview_events`; the SSE endpoint `LISTEN`s and re-broadcasts
mapped events, so multiple replicas each see every write.

## Sync daemon (CCT-601)

The daemon keeps a warm, GitHub-shaped cache so reads never touch GitHub.

### Runbook — environment

| Var | Default | Purpose |
| --- | ------- | ------- |
| `DATABASE_URL` | — | Postgres DSN. Unset ⇒ contract-only mode (no sync, empty store). |
| `GHREVIEW_SCHEMA` | `ghreview` | Dedicated schema inside the shared cctui database. |
| `GITHUB_TOKEN` | — | PAT used for octokit REST + GraphQL. |
| `GITHUB_ACCOUNT` | — | Account login the poller runs for. Unset ⇒ store + SSE only, no polling. |
| `GHREVIEW_POLL_INTERVAL_MS` | `30000` | Delay between poll sweeps. |
| `GHREVIEW_BUDGET_CEILING` | `0.2` | Fraction of the hourly rate budget the poller may spend. |
| `GHREVIEW_RATE_LIMIT` | `5000` | Per-PAT hourly request budget. |
| `GHREVIEW_WEBHOOK_SECRET` | — | Shared secret for `X-Hub-Signature-256` on `POST /v1/webhook`. |
| `PORT` | `8790` | HTTP port. |
| `GHREVIEW_SEAL_KEY` | — | 32-byte AES key (hex/base64/raw) that seals PATs at rest. Unset ⇒ accounts + poller disabled (store + auth only). Vault delivers it in prod (CCT-612). |
| `GHREVIEW_AUTH_MODE` | `cctui` | `cctui` verifies bearer tokens against the shared cctui DB; `static` uses `GHREVIEW_AUTH_TOKENS`. |
| `GHREVIEW_AUTH_TOKENS` | — | Static-mode `token:userId,token2:userId2` map (dev / standalone). |
| `GHREVIEW_CCTUI_SCHEMA` | `public` | Schema holding cctui's `auth_keys`/`users` for `cctui` auth mode. |
| `GITHUB_ACCOUNT` + `GITHUB_TOKEN` | — | Optional single-account bootstrap: seeds one `gh_accounts` row (owner `env`) when a seal key is set. Managing accounts via `/v1/accounts` is the multi-account path. |

## Multi-account, auth & isolation (CCT-603)

gh-review is a second backend beside the cctui Rust server. **AuthN reuses cctui's
bearer tokens**: cctui hashes tokens with `sha256(token)` and resolves them in
`auth_keys JOIN users`, and gh-review shares that Postgres, so the primary
(`GHREVIEW_AUTH_MODE=cctui`) resolver verifies a token with one query and no
network hop — the review UI mounted in cctui-ui (CCT-610) needs no second login.
A deliberately thin `static` mode (`GHREVIEW_AUTH_TOKENS`) keeps the service
standalone-capable and testable without a cctui DB. Auth is enforced on every
`/v1` route except `/v1/health`, `/v1/status`, `/v1/webhook` (HMAC-signed) and the
OpenAPI doc.

**Accounts.** `gh_accounts(id, user_id, login UNIQUE, encrypted_pat, poll/budget
overrides)` maps a user to N GitHub accounts. `login` is globally unique, so the
`documents`/`notification_state`/`sync_state` tables (keyed by `account` = login)
map 1:1 to exactly one owner. PATs are sealed with AES-256-GCM (`GHREVIEW_SEAL_KEY`)
— symmetric AEAD because the same service seals (on create) and opens (in the
poller); they are never returned by any API and never stored in plaintext.

**Isolation.** Every read/mutation is scoped at the storage layer by an ownership
predicate (`EXISTS (SELECT 1 FROM gh_accounts ga WHERE ga.login = account AND
ga.user_id = :user)`), so even a handler that forgets to filter cannot leak across
users; `subscriptions` carries an `account_id` FK. `GET /v1/events` is filtered to
the caller's set of logins. Per-account `poll_interval_ms` / `budget_ceiling` /
`rate_limit` overrides drive an `AccountManager` that runs one budgeted `Poller`
per active account and reconciles them as accounts are added/removed.

### Migrations

SQL files in `migrations/*.sql` are applied idempotently at boot by
`src/db/migrate.ts` (tracked in `ghreview.schema_migrations`, run in order, skipping
already-applied files). Tables: `subscriptions` (what to poll), `documents`
(envelope + JSONB payload with a GIN index, unique on `(account, kind, key)`),
`sync_state` (etags, cursors, rate snapshots), and `notification_state`
(read/done/archived flags per thread, with a partial index on `push_pending`).

### Polling budget

Every response updates a per-account `BudgetTracker` from the `x-ratelimit-*`
headers. **A `304 Not Modified` costs nothing against the GitHub rate limit**, so
only `200`/error responses increment `spent`. When `spent` reaches the ceiling
(`GHREVIEW_BUDGET_CEILING` × `GHREVIEW_RATE_LIMIT`, default 20% of 5000 = 1000/hour)
the sweep stops until the window resets. A secondary-rate-limit response
(`403`/`429` with `Retry-After`) forces a backoff window. This keeps sustained
polling of a warm PR set well under 20% of one account's budget: after the first
sync every unchanged PR returns `304` and is free.

Notifications polling honours `Last-Modified` / `If-Modified-Since` and the
`X-Poll-Interval` hint that the notifications API is designed around.

### Notification state (CCT-602)

GitHub's notifications API only models `unread`. On top of it we keep a
server-managed state layer in `notification_state` (one row per `(account,
thread_id)`): `read`, `done`, `archived` booleans with their timestamps. A row is
absent until first touched, so the inbox defaults everything to `false`.

- **Inbox feed** — `GET /v1/notifications` left-joins `documents(kind=notification)`
  with `notification_state` and returns cursor-paginated items shaped as an envelope
  plus a `state` object. Filters (all optional, combinable):
  - `reason` — GitHub reason (`review_requested`, `mention`, `ci_activity`); the
    aliases `review-requested` and `ci` are accepted.
  - `repo` — repository `full_name` (e.g. `DorskFR/cctui`).
  - `unread` — `true` shows only threads GitHub marks unread that are not locally
    read; `false` shows the read ones.
  - `undone` — `true` hides done threads; `false` shows only done ones.
  - `archived` — defaults to hiding archived; `true` shows only archived.
  - `since` — ISO timestamp; only notifications whose payload `updated_at` is at or
    after it (the age filter).
- **Mutations** — `POST /v1/notifications/state` (bulk, `thread_ids[]`) and
  `POST /v1/notifications/{id}/state` (single). Body carries `account` and any of
  `read`/`done`/`archived`; at least one is required. Both return the updated state
  per thread and emit `notification.updated`.
- **Two-way** — `read: true` is pushed back to GitHub via
  `PATCH /notifications/threads/{id}` (budget-aware, like the poller). `done` and
  `archived` are local-only. A read that has not yet been confirmed pushed keeps
  `push_pending = true` (with `last_error` on failure); the poller drains pending
  reads on every tick, so a push failure never loses the local flag. A re-polled
  notification upserts only the `documents` payload — it never clobbers state.

### Webhook (optional)

`POST /v1/webhook` verifies `X-Hub-Signature-256` (HMAC-SHA256 of the raw body with
`GHREVIEW_WEBHOOK_SECRET`) and upserts the payload exactly like a poll result.
Polling remains the universal path; the webhook is an optional latency shortcut for
org repos that can install one.

### GraphQL surface

GitHub's public GraphQL schema is vendored at `schema/github.graphql` (fetched from
`docs.github.com/public/fpt/schema.docs.graphql`, then run through
`scripts/sanitize-schema.ts` to drop a handful of spec-invalid `@deprecated`
directives GitHub ships that graphql-js rejects). `bun run gen:graphql` regenerates
`src/generated/github-graphql.ts` from the `src/graphql/*.graphql` operations. A thin
`createGraphqlClient` wrapper (`src/graphql/client.ts`) exposes the review-threads
query proving the surface; full GraphQL use lands in a later ticket.

## Development

Bun is provided via mise (`mise use bun@latest` in this directory installs it).

```sh
bun install
bun run dev        # hot-reloading server on PORT (default 8790)
bun run check      # typecheck + lint + test — the CI gate
```

### Generated artifacts (committed)

| Command | Output |
| ------- | ------ |
| `bun run gen:openapi` | `openapi.json` — OpenAPI 3.0.3 doc generated from the routes. |
| `bun run gen:client` | regenerates `openapi.json` **and** `src/generated/api.ts` (the TS client types via `openapi-typescript`). |
| `bun run gen:graphql` | regenerates `src/generated/github-graphql.ts` from the vendored GitHub schema + `src/graphql/*.graphql` operations. |

All are checked in. `bun run gen` refreshes the OpenAPI doc, the TS client and the
GraphQL types after any route/schema change; the contract test fails if the route
surface drifts.

The frontend (`cctui-ui`) consumes `src/generated/api.ts` — framework-agnostic
`paths`/`components` types it can pair with `openapi-fetch` or a thin `fetch`
wrapper, importing octokit types to narrow each envelope's `payload`.

## OpenAPI version — 3.0.3 (deliberate)

`@hono/zod-openapi`'s `app.doc()` uses the OpenAPI **3.0** generator, so the
emitted document is `3.0.3` (nullable via `nullable: true`, numeric constraints as
3.0 keywords). This is intentional and validated: `3.0.3` is the widest common
denominator for code generators — in particular `progenitor` targets 3.0.x. The
spec passes `redocly lint` structural validation (the `struct` / `nullable-type-sibling`
rules); the only remaining redocly findings are opinionated style rules
(`security-defined`, `operation-4xx-response`, `operation-operationId`,
`info-license`) that do not affect generator consumption.

## Rust client generation (validated once)

The future cctui-side consumer (Rust) is proven possible from this spec. Neither
`progenitor` nor `openapi-generator` (nor a JRE) is installed on the dev box, so
the invocation is documented rather than run in CI. The spec was validated as a
conformant OpenAPI document with `redocly lint` (the same structural checks both
generators require).

### Option A — progenitor (preferred; pure Rust, targets 3.0.x)

```sh
bun run gen:openapi                     # produce ghreview/openapi.json
cargo install cargo-progenitor          # one-time
cargo progenitor -i ghreview/openapi.json -o crates/ghreview-client -n ghreview-client --version 0.1.0
```

Spec notes for progenitor:
- progenitor consumes **OpenAPI 3.0.x** — this spec already emits 3.0.3, so no
  downgrade is needed.
- `payload` / `details` are untyped (`type: object` with no properties) and map to
  `serde_json::Value` — exactly the verbatim-JSONB passthrough we want.
- `/v1/events` (SSE) is documented for reference but is not an HTTP request/response
  operation; progenitor generates a client for the JSON routes and the SSE stream
  is consumed with a plain SSE client (e.g. `reqwest-eventsource`).

### Option B — openapi-generator (needs a JRE)

```sh
npx @openapitools/openapi-generator-cli generate \
  -i ghreview/openapi.json -g rust -o crates/ghreview-client \
  --additional-properties=packageName=ghreview-client,library=reqwest
```

openapi-generator 7.x supports both 3.0 and 3.1 input; the 3.0.3 doc works as-is.
