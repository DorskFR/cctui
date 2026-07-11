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
| GET | `/v1/repos` | List synced repositories (page of envelopes). |
| GET | `/v1/repos/{owner}/{repo}` | One repository envelope. |
| GET | `/v1/repos/{owner}/{repo}/pulls` | List synced PRs for a repo. |
| GET | `/v1/repos/{owner}/{repo}/pulls/{number}` | One PR envelope. |
| GET | `/v1/notifications` | Notifications inbox (page of envelopes). |
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

Single-account for this ticket; CCT-603 adds the multi-account model. The poller is
built around an `Account` abstraction (`src/github/account.ts`) so 603 slots in.

### Migrations

SQL files in `migrations/*.sql` are applied idempotently at boot by
`src/db/migrate.ts` (tracked in `ghreview.schema_migrations`, run in order, skipping
already-applied files). Tables: `subscriptions` (what to poll), `documents`
(envelope + JSONB payload with a GIN index, unique on `(account, kind, key)`), and
`sync_state` (etags, cursors, rate snapshots).

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
