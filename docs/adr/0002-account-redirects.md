# ADR 0002 — Account redirects: standing, expiring account/model overrides

- **Status:** Accepted — model redirects scoped to spawn time only (see Phasing)
- **Date:** 2026-08-27
- **Ticket:** CCT-835
- **Deciders:** cctui server maintainers
- **Relates to:** CCT-575 (explicit binding never falls back), CCT-735
  (per-family session account switching), CCT-757 (dollar soft limits),
  CCT-760 (per-account RPM/TPM), CCT-828/832

## Context

When an account's usage window is exhausted, the operator's loop today is
entirely manual and **per session**:

1. The gateway refuses a passthrough (`proxy.rs` soft-limit gate) or the
   provider returns a hard 429.
2. `mark_soft_limit_block` writes `sessions.soft_limit_reason` and publishes
   `SoftLimitReached`.
3. `ConversationDrawer` auto-opens `AccountSwitchModal`.
4. The operator picks another account (`POST /sessions/{id}/switch-account`).
5. The operator types **"continue"** — nothing resends the interrupted turn.

With N live sessions on the exhausted account, that is N × (4) + N × (5). The
operator already knows the answer before the first prompt appears: *"hirobot is
done for the day, use pafin until it resets."* There is no way to say that once.

### What exists to build on

The seams are unusually good — most of the machinery is already there.

- **Account model.** `accounts` (identity: name, owner, `env_json`) ×
  `account_providers` (one credential row per **family**, enforced by
  `UNIQUE (account_id, family)`; families are `anthropic | openai | fireworks`).
- **One name-resolution funnel.** `mint::account_provider_rows(state, user_id,
  account_name)` (`mint.rs:51`) resolves an identity by name — owned **or**
  shared via `resource_shares` — and returns its provider rows. Every launch
  path goes through it or through `mint_session_env*` which wraps it:
  `spawn.rs:196`, `spawn_child.rs:246`, `dispatch.rs:613/675`. It also backs
  `resolve_account_model`, so model aliases are resolved against the same row.
- **Switching a live session is one UPDATE.** `sessions::switch_account`
  (`sessions.rs:1919`) repoints `session_tokens.account_id` for a single family.
  The token *string* is unchanged, so the worker's `ANTHROPIC_AUTH_TOKEN` /
  `OPENAI_API_KEY` never changes and **no restart is needed**.
- **No caching on the read side.** `resolve_account` (`refresh.rs:80`) hits the
  DB on every proxied request, so a rebind takes effect on the very next call —
  including one made *during* the request that triggered it.
- **Limits are already modelled as windows** with reset times.
  `soft_limit.rs` normalises upstream usage into keys `session` (5h),
  `weekly_all`, `weekly_model:<id>`, `session_usd`, `usd_5h`, `usd_7d`, each
  carrying `{utilization, resets_at}`; `state.account_usage_cache` holds the
  latest per account. "Until the next reset" is therefore **computable**, not
  something the operator should have to type.

### What does not exist

- Any notion of a fallback, pool, priority, or ordered failover for accounts.
  Verified absent across `crates/*/src` and `migrations/`.
- **Hard-429 detection.** `gateway/mod.rs` is explicit: *"Status codes,
  `retry-after`, overload/streaming reconnects pass through untouched … No
  retries, no rate-limit handling."* Only cctui's *own* soft-limit 429 is
  observable. A real provider 429 (`anthropic-ratelimit-*`) is parsed nowhere.
- Any mechanism to resend an interrupted turn on the main session path. The
  `NUDGE_PROMPT` machinery in `agenttool.rs` is scoped to `CctuiAgent` children
  and explicitly refuses to nudge a child that errored.

### The two shapes the operator actually wants

The motivating examples are *not* the same shape, and conflating them produces a
worse design:

| Example | Source | Target | Same account? |
|---|---|---|---|
| "fable's limit is hit, use Opus" | account X, **model** `fable` | account X, **model** `opus` | yes — a **model** redirect |
| "hirobot is limited, use pafin" | account `hirobot` | account `pafin` | no — an **account** redirect |

Anthropic meters `weekly_model:fable` separately from `weekly_model:opus`, so
the first is a real, useful override that never leaves the account. A design
that only moves accounts cannot express it.

## Decision

Introduce **account redirects**: user-owned, optionally-expiring rules of the
form

```
(from_account, family, match_model?)  →  (to_account?, to_model?)   until expires_at?
```

resolved by one pure function and applied at two points. Everything else —
auto-arming, hard-429 handling, retry — is layered on this substrate later.

### Data model

`migrations/082_account_redirects.{up,down}.sql`:

```sql
CREATE TABLE account_redirects (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID        NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    from_account UUID        NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    to_account   UUID                 REFERENCES accounts(id) ON DELETE CASCADE,
    family       TEXT        NOT NULL,   -- anthropic | openai | fireworks
    match_model  TEXT,                   -- NULL ⇒ every model on this account
    to_model     TEXT,                   -- NULL ⇒ keep the requested model
    expires_at   TIMESTAMPTZ,            -- NULL ⇒ until explicitly removed
    reason       TEXT,                   -- free text / auto-arm provenance
    armed_by     TEXT        NOT NULL DEFAULT 'manual',  -- manual | auto
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX account_redirects_uniq
    ON account_redirects (from_account, family, COALESCE(match_model, ''));
CREATE INDEX account_redirects_live
    ON account_redirects (from_account, family)
    WHERE expires_at IS NULL OR expires_at > now();

ALTER TABLE account_redirects
    ADD CONSTRAINT account_redirects_not_identity
        CHECK (to_account IS DISTINCT FROM from_account OR to_model IS NOT NULL),
    ADD CONSTRAINT account_redirects_has_target
        CHECK (to_account IS NOT NULL OR to_model IS NOT NULL);
```

Notes:

- `to_account NULL` ⇒ a **model-only** redirect (the fable→opus case) — stay on
  the account, change the model.
- `to_model NULL` ⇒ a **pure account** redirect (the hirobot→pafin case).
- Both set ⇒ "hirobot's opus is gone, use pafin's sonnet".
- The unique index makes a rule **idempotent per (source, family, model)**:
  re-arming overwrites rather than stacking, which is what keeps the auto-arm
  path in phase 3 from accumulating garbage.
- `family` is on the rule, not derived, because a session can hold an anthropic
  *and* an openai binding at once; redirecting one must not touch the other.
  This is the same invariant `switch_account:1917` and `mint_env_for_account:326`
  already protect with an explicit family predicate.
- Rows are not deleted on expiry. `expires_at` is a filter, so history survives
  for the UI ("hirobot → pafin, expired 09:00") and a nightly sweep can prune.

### The resolver

One pure-ish function, unit-testable, in a new
`crates/cctui-server/src/routes/gateway/redirect.rs`:

```rust
pub struct RedirectTarget {
    pub account_id: Uuid,        // accounts.id (identity), == input when model-only
    pub model: Option<String>,
    pub hops: Vec<Uuid>,         // provenance for logging / the UI
}

pub async fn resolve_redirect(
    state: &AppState,
    user_id: Uuid,
    from_account: Uuid,
    family: Family,
    model: Option<&str>,
) -> Option<RedirectTarget>;
```

Rules:

1. Follow rules transitively (A→B, B→C ⇒ A→C) with a **visited set** and a hard
   depth cap of 4. A cycle resolves to the last non-repeating hop; it never
   loops and never errors out a launch.
2. A more specific rule wins: `match_model = <m>` beats `match_model IS NULL`.
3. The target must have a provider row in `family` and must be **usable by
   `user_id`** (owned or shared) — the same predicate `account_provider_rows`
   already applies. A target that fails this is skipped as if the rule did not
   exist; a redirect must never widen access.
4. Any resolution failure degrades to "no redirect". A broken rule must never
   fail a spawn — the same fail-soft posture `resolve_account_model` takes.

### Application point 1 — bind time (all *future* sessions)

Insert the indirection in `mint::account_provider_rows`, immediately after the
name→identity lookup and before the provider-row fetch. That single edit covers
`spawn.rs`, `spawn_child.rs` and `dispatch.rs`, because all three resolve an
account **by name** through this function.

Consequences, all of them desirable:

- The session is bound to the target from the start. `sessions.account_id`,
  `session_tokens`, the accounts page, cost attribution and `request_count` all
  name the account actually being billed — no phantom bookkeeping.
- The target's `env_json`, `settings_json` and `model_aliases` are the ones
  minted into the worker, because minting happens *after* the redirect.
- `resolve_account_model` resolves the model against the target's alias map,
  which is the correct behaviour when the accounts have different catalogs.

`to_model` is applied here too, by threading the resolved target into
`resolve_account_model`. **Model redirects apply only at bind time** — see
Consequences for why not at the proxy.

### Application point 2 — request time (moves *live* sessions)

In `proxy.rs::passthrough`, immediately after `resolve_account` succeeds and
**before** the soft-limit gate:

1. Resolve a redirect for `(acct → identity, family)`.
2. On a hit, perform the same one-line repoint `switch_account` does — factored
   out into `store::tokens::rebind_session_family(pool, session_id, family,
   target_provider_id)` and called by both.
3. Re-run `resolve_account` (or load the target row directly) and continue the
   **current** request on the new credentials.
4. `clear_orphan_block_for_session` + `clear_soft_limit_block`, and publish a
   new `ServerEvent::SessionAccountRedirected { session_id, from, to, reason,
   expires_at }`.

Because the token string is unchanged and `resolve_account` is uncached, this is
transparent: the worker never sees a 429, the turn is never interrupted, and
**nobody has to type "continue"**. That is the whole point of the design.

A rebound session **stays** on the target when the rule expires. Moving a live
session back mid-turn is more disruptive than the drift it prevents, and the
operator can move it by hand. The rule's expiry governs *new* sessions only.

### API and events

- `GET    /api/v1/accounts/{id}/redirect` — the live rule, if any.
- `PUT    /api/v1/accounts/{id}/redirect` — `{ to_account?, family, match_model?,
  to_model?, until? }` where `until` accepts an RFC3339 timestamp **or** the
  token `"next_reset"`, resolved server-side from
  `state.account_usage_cache` → the blocking window's `resets_at`.
- `DELETE /api/v1/accounts/{id}/redirect`
- `GET    /api/v1/redirects` — all live rules for the user (drives a global
  banner).
- `ServerEvent::SessionAccountRedirected` (new) and reuse of
  `SoftLimitCleared`.

`"next_reset"` matters: it is the difference between "set a redirect" being one
click and being a datetime-picker chore.

### UI

On the accounts screen (see the companion full-width redesign — a 22 rem card
has no room for this):

- A **Redirect** control per account card: `hirobot → pafin · until 14:00 ✕`,
  rendered as a badge on the card header when live.
- Per usage bar, a "when this window is exhausted → …" target picker. That is
  where a `weekly_model:fable → opus` rule is created in context, with
  `match_model` and `until = next_reset` pre-filled from the bar itself.
- `AccountSwitchModal` gains a "…and redirect all future sessions" checkbox, so
  the *next* time the operator does the manual switch, it is the last time.

## Phasing

**Phase 1 — substrate + manual rules (bind time only).** Migration,
`store/account_redirects.rs`, `redirect.rs` resolver + unit tests (specificity,
transitivity, cycles, depth cap, family scoping, access check), hook into
`account_provider_rows`, CRUD API, accounts-page UI. Delivers "set a temporary
redirection for all future sessions" end to end. No proxy changes, no behaviour
change for live sessions.

Model redirects (`to_model`) exist **only** here: a rule can flip the model a
*new* session spawns with (including detection-driven arming in phase 3), but no
phase ever changes the model of a running session. Live-session handling below
is account-only.

**Phase 2 — live sessions.** Factor `rebind_session_family` out of
`switch_account`, apply the resolver in `proxy.rs`, add
`SessionAccountRedirected`, and have `ConversationDrawer` show an inline note
instead of auto-opening the switch modal when a redirect handled it.

**Phase 3 — auto-arm.** Add `accounts.overflow_account_id`. When a limit is
observed, arm a rule automatically with `expires_at = window.resets_at`:

- from the soft-limit `Decision::Block` (already has `key`, `reason`,
  `retry_after_secs`) — free, no new detection;
- from a **hard upstream 429**, which requires the one genuinely new piece of
  detection: parse the Anthropic error envelope / `anthropic-ratelimit-*`
  headers for `resets_at` on non-2xx responses. Cheap, because it only runs off
  the success path.

At that point the operator configures an overflow target **once** and never sees
the prompt again.

**Phase 3b (optional) — one-shot retry.** On a hard 429 that a redirect can
rescue, replay the request against the target so even the first interrupted turn
survives. This needs the request body buffered, which the proxy currently
streams. Gate the buffering on "this account has a live redirect **or** an
overflow target", so the hot path stays zero-copy for everyone else — the same
conditional-buffer pattern `proxy.rs:330` already uses for Fireworks/Anthropic
body rewrites.

## Consequences

**Good**

- One place to express intent, N sessions obey it.
- Reuses proven primitives: the repoint is the exact UPDATE `switch_account`
  has been doing since CCT-735; the resolution funnel is the one every launch
  path already shares.
- Fails soft everywhere. A bad, expired, cyclic or inaccessible rule degrades to
  today's behaviour rather than breaking a launch.
- Phase 1 is independently useful and touches no request-path code.

**Costs / risks**

- **An extra query on the launch path.** `account_provider_rows` gains one
  indexed lookup per resolution. Launches are rare; acceptable. The proxy-side
  lookup (phase 2) is on the hot path and must be gated by an in-memory
  "any live redirects for this user" flag, mirroring the
  `state.soft_limit_blocked.is_empty()` and `state.session_usd_budgets.is_empty()`
  guards already used to keep cold paths free.
- **Indirection is confusing if invisible.** A session bound to `pafin` when the
  operator typed `hirobot` must say so. The UI badge and
  `SessionAccountRedirected` are not optional polish; they are what keeps this
  from being spooky action at a distance. Log every applied redirect with its
  `hops`.
- **Model rewriting at the proxy is rejected.** Rewriting `model` in the request
  body would be silent to the harness: Claude Code would display `fable` while
  being served `opus`, with different cost and behaviour. Model redirects
  therefore apply at bind time only, where the session genuinely starts on the
  target model and the harness knows it. (The mechanism is available if we ever
  decide the trade is worth it — the conditional body-rewrite path exists.)
- **Shared accounts.** Rules are per-user, so two users sharing `hirobot` can
  hold different redirects. That is correct — the limit is shared but the
  overflow preference is personal — but it means the accounts page must scope
  the badge to the viewer.
- **Cross-family redirects are out of scope**, matching `switch_account`'s
  existing 409: an anthropic binding cannot be redirected to an openai account.
  The worker's env keys are family-specific.

## Alternatives considered

- **Ordered account pools** ("try hirobot, then pafin, then …"). More general,
  but it replaces an explicit operator decision with an implicit policy, needs
  health tracking per member, and makes "which account am I on?" genuinely hard
  to answer. The redirect rule is a strictly simpler primitive that a pool
  could later be built from.
- **Redirect at `resolve_account` without rebinding** (per-request credential
  substitution). Simplest to implement, but `sessions.account_id`,
  the accounts page, and cost attribution would all keep naming the source
  account while the target is billed. Rejected — silent divergence between what
  the UI says and what is charged is the worst failure mode here.
- **Automating the existing modal** (auto-click "switch" on `SoftLimitReached`).
  Requires no new model, but only reacts *after* a turn has already been
  interrupted, so the operator still types "continue". It also cannot express
  the model-level case at all.
- **Doing nothing and adding a bulk "switch all sessions on account X" button.**
  Helps the N-sessions problem, does nothing for future sessions, and still
  needs N "continue"s.
