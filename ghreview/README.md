# gh-review

Versioned HTTP + SSE contract for the cctui GitHub review center (epic CCT-600).

This package is the **contract surface only** (CCT-604). It is a Bun + TypeScript
service built on [Hono](https://hono.dev) + [`@hono/zod-openapi`](https://github.com/honojs/middleware/tree/main/packages/zod-openapi):
the `/v1` routes and their zod schemas are the source of truth, and the OpenAPI
document + TypeScript client are generated from them.

Handlers return stubbed/empty data — the sync daemon that fills the store lands in
CCT-601. The shapes, pagination params, error model and event catalogue are the
real, frozen contract.

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
Later tickets (CCT-601/602) wire these to Postgres `LISTEN/NOTIFY`.

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

Both are checked in. `bun run gen` (alias for `gen:client`) refreshes both after
any route/schema change; the contract test fails if the route surface drifts.

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
