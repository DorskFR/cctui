# GitHub integration — architecture & breakdown (design spec)

> Status: **partially built, partially superseded.** This was the original design
> spec for a GitHub integration living inside `cctui-github`. Epic 0 (plugin
> skeleton) and Epic 1 (connector, webhook, reconcile, classifier feed) shipped
> as specified. The fast diff viewer and the review-draft/publish UI (Epic 2, and
> the UI half of Epic 3) were superseded by a standalone service, **ghreview**
> (`ghreview/` + `ghreview-ui/`, epic CCT-600) — see `ghreview/README.md`. Those
> sections below are kept for the parts still current: the connector/webhook/
> capability design (§6.1, §7) and the agent MCP review-draft tool (§6.3, still
> in `cctui-github`), and are historical for the removed diff-viewer plan (§6.2).

## 1. Goals

1. **Stop missing things.** A real-time, low-latency view of "PRs that need me":
   PR state, new commits, review requests, CI checks failing, new
   comments/threads, merge/conflict state — updating live so action latency drops
   from "noticed 2h later" to "noticed now".

2. **Review efficiently.** A fast, controlled diff reviewer: only what changed,
   file-by-file / hunk-by-hunk, **fully virtualized** so big PRs render instantly,
   with fast inline commenting. Stop loading the GitHub web UI for large diffs.

3. **Review *with* an agent.** Start a review session on a chosen machine that
   reviews the PR; use the same session to ask inline questions about a block and
   to refine review comments *before* posting them to GitHub (draft in cctui,
   publish in one batch).

Then: **make all of this optional and cleanly removable** — a cctui that isn't
using GitHub should not grow a nav item, routes, background work, *or database
state*. Adding it is easy; removing it leaves **no stale rows**.

### Non-goals (v1)

- Not a general GitHub client (no issue triage UI, no Actions management, no repo
  browsing). Scope is **PRs + review**.
- Not GitLab/Bitbucket (the model generalizes; only GitHub is built).
- Not multi-tenant SaaS hardening. cctui is effectively single-user / homelab.

## 2. Two firm constraints from review (these shape everything below)

- **The daemon stays a session-messaging agent only.** `cctui-daemon` is *not* a
  relay for GitHub review data (no serving diffs/blobs/git ops to the review UI).
  It does what it does today: spawn/observe agent sessions and carry their
  messages. All GitHub data flows **server ↔ GitHub** and **server ↔ webui**.
- **Diffs come from GitHub only.** No local checkout is required to render a diff.
  This is proven by **DiffsHub** (Pierre) — `diffshub.com/<org>/<repo>/pull/<n>`
  renders any public GitHub PR fully virtualized, sourced entirely from GitHub.
  cctui's server proxies/caches that data; the webui virtualizes it.

## 3. Inspiration, distilled (what we take, what we don't)

- **DiffsHub / diffs.com / mblode/diffhub** — the diff-viewer bar and proof that
  **GitHub-only, fully virtualized** works. diffhub's "single virtualized surface
  that scales to thousands of files" is the rendering model to copy. The known
  edge: GitHub serves >100k-line diffs unreliably (delayed first byte) — engineer
  for it (streaming/pagination, large-file blob fallback, per-SHA cache).
- **richelieu** (`MiLk/richelieu`) — the **connector** concept (sync external
  sources), **repo-scoped prompts**, and review-as-a-first-class-item. cctui
  already owns the *execution* half (spawn/observe agents); we add the
  **connector + sync** half. We adopt its vocabulary (*connector*, *prompt
  scoping*).
- **slop-review** (`genkio/slop-review`) — **inspiration only; not adopted as
  is.** We are *not* using its `.reviews/` file convention or its skill. We keep
  two ideas, implemented natively in cctui: (a) **draft locally, publish as one
  batched review** (refine before posting); (b) **per-file "reviewed" marks keyed
  to the blob SHA** so a later push only re-flags files that actually changed.
- **pierre / diffs.com** — keyboard-driven, instant. The speed/UX bar.

## 4. What already exists in cctui (the seams we build on)

- **`SessionChild { id, href, kind: "pr" }`** (`cctui-proto/src/adapter.rs`) +
  the **classifier** PR status cache (`cctui-proto/src/classifier.rs`): sessions
  already link PRs and bucket into *Ready for review*. The connector can enrich
  that cache; if GitHub is uninstalled the link is just opaque core metadata —
  harmless.
- **Trigger stub** `POST /api/v1/triggers/{kind}` (501 today; migration
  `017_triggers_stub.sql`) — the intended webhook ingress. The public server is a
  natural webhook target.
- **Credential vault** `oauth_accounts` (migration `034`) — encrypted, per-user,
  per-provider creds via `crate::crypto`. We reuse the *pattern* (not the table)
  for connector credentials.
- **Spawn / dispatch / reply** (`routes/spawn.rs`, `routes/dispatch.rs`,
  `AdapterCommand::Spawn`/`Reply`) — a review session is a normal session.
- **Client broadcast WS** `ServerEvent` (`cctui-proto/src/ws.rs`) — the live push
  channel; new PR/review events ride it.
- **Rust→TS bindings** via ts-rs (`webui/src/lib/bindings/`).
- **Webui**: SvelteKit static SPA (Svelte 5 runes + Tsumikit + TanStack Query);
  views are route files; `onStream`/`onPerms`/`onAsk` callbacks feed
  component-local `$state`. **No plugin runtime and no UI feature-gating exist
  today** — both are introduced here in their cheapest viable form (§7).

## 5. Architecture overview

One server-centric data plane. The daemon appears only as the thing that runs
agent sessions (unchanged role).

```
              ┌───────────────────────── GitHub ─────────────────────────┐
              │ webhooks (push)           REST/GraphQL (pull/reconcile,     │
              │                           PR diff, blobs, review submit)    │
              └────▲─────────────────────────────▲───────────────────────────┘
   POST /triggers/github                          │
        ┌─────────┴───────────────────────────────┴──────────────────────────┐
        │                         cctui-server                                 │
        │   ┌──────────────────────── cctui-github crate ─────────────────┐    │
        │   │ connector (creds, webhook verify, reconcile poll)           │    │
        │   │ PR state store      diff proxy/cache     review-draft store  │    │
        │   │  (schema: github.*  — see §7 uninstall)                      │    │
        │   └───────┬───────────────────┬──────────────────────┬──────────┘    │
        │           │ broadcast          │ HTTP /api/v1/github   │ MCP review    │
        │           ▼                    ▼                       ▼  tool         │
        │     ServerEvent (WS)     diff/draft JSON         (writes drafts)       │
        │           │                    │                       ▲              │
        │   ┌───────┴──────── existing: sessions, dispatch, classifier ────┐    │
        │   └───────────────────────────┬───────────────────────┬─────────┘    │
        └───────────────────────────────┼───────────────────────┼──────────────┘
              client WS (live) │         │ dispatch (spawn agent) │ session msgs
        ┌────────────────────┴──┐   ┌────┴───────────────────────┴───────────┐
        │ webui /github view     │   │ daemon (machine) — session messaging    │
        │ • PR inbox (live)      │   │ ONLY. spawns the review agent, carries  │
        │ • virtualized diff     │   │ its messages. No GitHub data relay.     │
        │ • draft → publish      │   │                                          │
        │ • block→ask agent      │   │ (agent may `gh pr checkout` itself if it │
        └────────────────────────┘   │  wants to run code — that's its own work)│
                                      └──────────────────────────────────────────┘
```

## 6. The three components

### 6.1 Component A — connector & dashboard ("stop missing things")

**Credentials.** A `github.connectors` row holds an encrypted GitHub credential
(GitHub App installation token *or* fine-grained PAT) + config (repos/orgs,
webhook secret). Auth model decision in §8 — **GitHub App recommended** (proper
webhooks, scoped, short-lived tokens); PAT + hand-registered webhook is fine for
an MVP spike. The webui and agents never see the credential.

**Ingestion = webhook + reconcile** (mirrors richelieu's SyncLoop, but push-first):
- **Webhook:** implement `POST /api/v1/triggers/github`; verify
  `X-Hub-Signature-256`; handle `pull_request`, `pull_request_review`,
  `pull_request_review_comment`, `issue_comment`, `check_suite`/`check_run`,
  `status`, `push`. Each upserts `github.*` rows and emits a `ServerEvent`.
- **Reconcile poll:** a background loop (mirror the `main.rs` reaper) queries "PRs
  involving me" (`review-requested:@me` / `author:@me` / etc. — scope decision in
  §8) every N seconds + on connector start, healing missed webhooks and hydrating
  first install. Conditional requests / ETags for rate limits.

**Stored state** (all in schema `github`): `pulls`, `checks`, `reviews`,
`review_threads`, `review_comments` (the posted side). Connector derives an
**attention bucket** per PR (mirrors the session classifier): *Needs my review*,
*My PR — changes requested*, *My PR — CI red*, *My PR — mergeable*, *Waiting*.

**Live push:** new `ServerEvent` variants (`GithubPullUpdated`,
`GithubCheckUpdated`, `GithubReviewActivity`, or one `GithubEvent` envelope). The
`/github` inbox subscribes with the existing callback→`$state` pattern.

**Classifier tie-in (free win):** the connector publishes check/review state into
the PR status cache the classifier already reads, so an agent session that opened
PR #N flips to *Ready for review* / *CI red* in the **Sessions** view with no
extra UI.

### 6.2 Component B — fast diff viewer ("review efficiently") — superseded

This component (server diff proxy, virtualized viewer, review-draft/publish HTTP
routes and their `github.*` tables) was originally planned to live in
`cctui-github`, and an early version shipped there. It was later removed
(CCT-611) in favour of a standalone service, **ghreview** (`ghreview/` backend +
`ghreview-ui` frontend, epic CCT-600) — see `ghreview/README.md` for the current
diff-viewer and review-publish design.

### 6.3 Component C — agent review sessions ("review *with* an agent")

A review session is a **normal cctui session** (daemon spawns it and carries its
messages — its only role). No daemon GitHub relay.

**Flow:**
1. From the PR inbox / PR detail: **"Review with agent"** → the existing spawn
   modal, PR context prefilled (machine/dispatcher, model/effort, **repo-scoped
   review prompt** — extend the `prompts` table with repo scoping, richelieu-style
   most-specific-wins).
2. The session is seeded with the PR context (diff or a pointer + the ability to
   fetch via `gh`). If the agent wants to run code/tests it can `gh pr checkout`
   itself — that's the agent's own tool use, **not** a daemon data path.
3. The agent writes **draft comments into cctui's review store** via a small
   **MCP review tool** (`review_comment`, `review_summary`) authenticated with the
   session token, writing to `github.review_draft_comments`. (This replaces any
   slop-review file/skill mechanism.)
4. The session auto-links its PR via the existing `SessionChild { kind:"pr" }`.

**The block↔conversation bridge (headline UX):** diff viewer + existing
conversation drawer, side by side. Per your steering, the agent does **not** need
a checkout to discuss a block — we **send it the snippet, or the file + line
chunk, as message context**:
- Select a block → **"Ask the agent about this block"** injects a message (reuse
  `ws.sendMessage`) carrying `path`, line range, and the snippet text.
- The agent's answer can be **promoted to a draft comment** anchored to that block
  with one action.
- Human curates drafts; nothing reaches GitHub until **Publish**.

**Reuse ledger** — reused as-is: spawn/dispatch, `Reply`/`Interrupt`,
conversation streaming, permission/ask/plan cards, `SessionChild` linking,
prompts. New: repo-scoped prompt selection, the MCP review tool, the block→message
glue, the publish action.

## 7. The plugin: easy to add, easy to remove, no stale state

This is the part you care most about. cctui has no plugin runtime today, so the
design goal is the cheapest thing that delivers **true optionality + clean
teardown**, fully encapsulated in its own crate.

### 7.1 Code: one crate, `cctui-github`

Everything GitHub lives in `crates/cctui-github`: connector + GitHub client,
webhook handler, the agent review-draft store, connector HTTP routes
(`/api/v1/github/connectors*` + `/api/v1/triggers/github`), the MCP review
tool, proto additions (re-exported), and **its own embedded migrations**. The
diff proxy and viewer live outside this crate now, in `ghreview` (§6.2). The
server's `main.rs` mounts `cctui-github` behind a
`github` **Cargo feature**:

```rust
#[cfg(feature = "github")]
{ cctui_github::migrate(&pool).await?; router = router.merge(cctui_github::routes(state.clone())); }
```

A build without the feature contains **zero** GitHub code/routes/columns.

### 7.2 Database: a dedicated schema + one-directional FKs

The decisive design choice for "no stale entries":

- **All GitHub tables live in a dedicated Postgres schema `github`** (`github.connectors`,
  `github.pulls`, `github.checks`, `github.review_drafts`, …).
- **FKs may point *from* `github.*` *into* core (e.g. `connectors.user_id →
  users.id`), but core never FKs into `github.*`.** Because a FK constraint lives
  on the *referencing* table, `DROP SCHEMA github CASCADE` removes every GitHub
  table **and** its outbound constraints **without touching core tables**. Core
  has zero knowledge of GitHub.
- The crate's migrations run with `search_path = github`, so even sqlx's own
  `_sqlx_migrations` bookkeeping lands in `github._sqlx_migrations` — independent
  of core's migration history, and dropped with the schema. *(Validate this sqlx
  behaviour early — it's the kind of detail that bites; ticket GH-PKG-2.)*

### 7.3 Install / uninstall lifecycle

- **Install** (no rebuild needed if shipped feature-on-but-dormant): the user adds
  a connector in the UI → the crate's migrations have already created the `github`
  schema on start → capability flips on → the nav item + view + actions appear.
  ("Heavier" install = enable the Cargo feature and build; only needed if you want
  to exclude the code entirely.)
- **Uninstall** = one owner action **"Remove GitHub integration"** that:
  1. best-effort deregisters the GitHub webhook,
  2. deletes connectors + the encrypted credential,
  3. `DROP SCHEMA github CASCADE` — **all** GitHub state gone, core untouched,
  4. capability flips off → the webui hides the nav item, route, and every
     contextual action.
  Afterwards you may rebuild without the Cargo feature to drop the code too. No
  stale rows, no orphaned migrations, no dangling FKs.

### 7.4 UI gating

- Server exposes **`GET /api/v1/capabilities`** → `{ github: { enabled, repos } }`
  (enabled = crate present **and** schema exists **and** a connector configured).
- The webui adds one small `capabilities` store; it conditionally mounts the
  **`/github` route** (lazy-loaded so non-GitHub users never download the heavy
  diff viewer) and the **contextual actions** (session → "Open as PR review",
  "Link PR #…"). This introduces the capability-gated-view pattern the webui
  currently lacks — the one primitive a future real plugin system would also need.

### 7.5 Graceful degradation when absent

The classifier's PR cache is best-effort; without GitHub, sessions still render
(just no enriched PR status). `SessionChild` links are opaque core metadata and
keep working. Nothing in core depends on the crate.

## 8. Open decisions (need a call before building)

1. **GitHub auth model** — GitHub App (proper webhooks, scoped, short-lived
   tokens; more setup) vs fine-grained PAT + hand-registered webhook (fastest MVP,
   single identity). *Rec: App for the real thing; PAT acceptable for an Epic-1
   spike.*
2. **Scope of "involves me"** — authored / review-requested / assigned /
   mentioned / team-review-requested. Defines the reconcile query and the inbox.
   *Rec: authored + review-requested (direct & team) for v1.*
3. **Connector teardown UX** — owner-only "Remove integration" button (per §7.3)
   vs CLI/admin-only. *Rec: owner button + a `cctui-admin` command.*

*(Resolved by your steering: diffs are GitHub-only — no daemon relay, no
working-copy lens; the agent gets block context via snippet/file+line, not a
served checkout; plugin = `cctui-github` crate + `github` schema, runtime
capability-gated UI, drop-schema uninstall.)*

## 9. Proto / wire additions (summary)

- **`ServerEvent`** (client WS): `GithubPullUpdated` / `GithubCheckUpdated` /
  `GithubReviewActivity` (or one `GithubEvent { kind, payload }` envelope).
- **HTTP** `/api/v1/github/connectors*` (CRUD) plus `triggers/github` (webhook)
  and `GET /api/v1/capabilities`. The pulls/diff/publish/mark-viewed routes
  originally planned here shipped, then moved to the standalone `ghreview`
  service's own `/v1` contract (§6.2) — they are not part of `cctui-github`.
- **MCP tool** (agent side): `review_comment`, `review_summary` — writes agent
  drafts into `cctui-github`'s own draft store (kept; not part of the ghreview move).
- **No new daemon frames.** The daemon is untouched (session messaging only).
- All new proto types get ts-rs bindings → webui automatically.

## 10. Phased delivery & ticket breakdown (actionable)

Sized rough (S ≈ <1d, M ≈ 1–3d, L ≈ 1wk+). Deps in parentheses.

### Epic 0 — Plugin skeleton (do first; everything hangs off it)
- **GH-PKG-1 (M):** create `cctui-github` crate behind a `github` Cargo feature;
  mount routes + run embedded migrations from `main.rs` under `#[cfg]`.
- **GH-PKG-2 (M):** `github` schema + `search_path`-isolated migrator; verify
  sqlx tracking lands in `github._sqlx_migrations`; implement & test
  `DROP SCHEMA github CASCADE` uninstall leaving core intact.
- **GH-CAP-1 (S):** `GET /api/v1/capabilities` + webui `capabilities` store +
  conditional, lazy-loaded `/github` nav item & route.

### Epic 1 — Connector & dashboard ("stop missing things") — *first real value*
- **GH-CONN-1 (M):** `github.connectors` + crypto credential + CRUD + connector
  setup UI. *(GH-PKG-2; auth decision §8.1)*
- **GH-CONN-2 (M):** `POST /api/v1/triggers/github` webhook (sig verify + routing).
- **GH-CONN-3 (M):** `github.pulls`/`checks`/`reviews` + upsert from events.
- **GH-CONN-4 (M):** reconcile poll loop + rate-limit/ETag + first-install hydrate.
- **GH-CONN-5 (S):** `ServerEvent` variants + broadcast on upsert.
- **GH-CONN-6 (S):** attention-bucket derivation per PR.
- **GH-UI-1 (M):** `/github` PR-inbox view — live, grouped by bucket, reusing
  SessionCard-style rows + filters. *(GH-CONN-5, GH-CAP-1)*
- **GH-CLS-1 (S):** feed connector state into the classifier PR cache. *(GH-CONN-3)*

### Epic 2 — Fast diff viewer ("review efficiently") — superseded (CCT-611)

Shipped as GH-VIEW-1..6 inside `cctui-github`, then removed in favour of the
standalone `ghreview` service (§6.2). See `ghreview/README.md` and its own
ticket history (epic CCT-600) for the current design.

### Epic 3 — Agent review sessions ("review with an agent")
- **GH-AGENT-1 (M):** repo-scoped review-prompt selection (extend `prompts`) +
  "Review with agent" entry points wired to the spawn modal with PR context
  prefilled. *(spawn/dispatch — exists)*
- **GH-AGENT-2 (M):** MCP review tool (`review_comment`/`review_summary`) writing
  to the draft store with the session token.
- **GH-AGENT-3 (M):** block↔conversation bridge — "ask agent about this block"
  (inject `path`+lines+snippet) + "promote answer to draft comment". Needs a
  diff surface to select a block from — now `ghreview-ui`, not GH-VIEW-3.

**Order:** Epic 0 → Epic 1 (triage value, no machine) → Epic 3 (agent loop). Epic
2 is superseded by `ghreview`. Epic 1 alone is independently shippable.

## 11. Risks

- **Large diffs (>100k lines).** GitHub serves them unreliably (delayed first
  byte) — now `ghreview`'s concern (needs streaming/pagination + blob fallback +
  a UI affordance).
- **Rate limits / missed webhooks.** Reconcile loop + conditional requests are not
  optional; webhooks alone drift.
- **Uninstall correctness.** The `github`-schema + one-directional-FK invariant is
  what makes `DROP SCHEMA … CASCADE` safe — a core→`github` FK would silently
  break it. Enforce/lint the invariant; test teardown leaves core untouched.
- **Public-repo hygiene.** Keep all secrets in the connector vault; never log
  tokens/payloads; no homelab specifics in committed code.
- **Webui bundle growth.** Lazy-load `/github` so non-GitHub users don't pay for
  the heavy viewer (supports the "optional" promise).
```
