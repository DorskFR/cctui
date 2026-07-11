# pulldash idea audit — patterns to steal for the gh-review center (CCT-606)

Systematic pass over `coder/pulldash`, audited at commit **`1fd065e`** ("feat: add
personal access token authentication (#15)", 2026-01-05), cloned to
`~/.claude/artifacts/pulldash-audit/pulldash`. Part of epic **CCT-600**.

pulldash's thesis: "Fast, filterable PR review. Entirely client-side." It runs with
**no backend** — GitHub's REST/GraphQL API supports CORS, so the browser talks to
GitHub directly and does every expensive thing (diff parse, syntax highlight,
navigation indexing, request caching) on the client. The tiny Hono server
(`src/node/main.ts`) only serves static assets, does SPA fallback, and proxies the
two OAuth device-flow endpoints that lack CORS.

**Our architecture is the inverse.** The gh-review center has a warm server-side
sync daemon (CCT-601) with a Postgres JSONB store, ETag polling, and SSE push, plus
a versioned `/v1` contract (CCT-604). Performance requirement #1 is **instant opens
from a warm backend**. So pulldash's heroics around client-side caching, request
dedup, and SWR are, for us, mostly a *catalogue of problems the backend deletes* —
we read them to know what NOT to rebuild. What we do steal is the **UX semantics and
the rendering pipeline**: the parts that stay client-side no matter where the data
comes from.

> **License: pulldash is AGPL-3.0-only.** Everything below is a description of
> observed *behavior* in our own words. Do not copy source. Trivial identifiers,
> route shapes, and localStorage key ideas are the only literal carryover, and even
> those we should rename to our own conventions. See the compliance note at the end.

---

## 1. Tab system (→ CCT-605 foundation)

**What pulldash does.** A single `TabProvider` (React context, `useState` +
`useEffect`-persisted to `localStorage` under one key) holds an array of tabs plus an
`activeTabId`. Tab #0 is a permanent, non-closable "Home" tab. PR tabs get a
deterministic id derived from the PR coordinates (`pr-<owner>-<repo>-<number>`), so
"open this PR" is idempotent — opening an already-open PR just re-activates its tab
rather than duplicating it. On load, persisted state is validated: a Home tab is
re-inserted if missing, and `activeTabId` falls back to Home if it points at a tab
that no longer exists. Closing the active tab selects the adjacent one
(`min(index, len-1)`).

**URL is the source of truth.** The router has exactly two routes: `/` and
`/:owner/:repo/pull/:number`. An effect in the shell reads the route params and
reconciles tabs → URL both ways (navigating creates/activates the matching tab;
clicking a tab calls `navigate()`). Only the **active** PR tab is mounted; inactive
PR tabs are unmounted entirely ("only render active tab to avoid parallel data
fetching"). Home stays mounted but `invisible` because it's cheap.

**Live status per tab.** Each tab carries a `status` object — `{ checks, state,
mergeable }` — that the *tab content* pushes up via `updateTabStatus(tabId, …)`
whenever its PR data or CI status changes. A small colored dot renders the rollup:
purple merge icon when merged, red for closed/conflicts/failing-checks, yellow for
checks-running, green for ready, a pulsing grey dot while loading. `Cmd/Ctrl+1..9`
switches tabs; `Cmd/Ctrl+W` closes the current one (never Home); middle-click closes.

**Why it works.** Deterministic ids make idempotency trivial and let the URL, the
tab list, and localStorage all agree without a reducer. Rendering only the active tab
keeps N open PRs from each spinning up their own fetch/parse pipeline. The status dot
is a genuinely nice at-a-glance signal across many open reviews.

**Adopt.** The whole model: permanent Home tab, deterministic PR-coordinate tab ids,
localStorage restore with validation/fallback, active-tab-only mounting, the
`Cmd+number` / `Cmd+W` / middle-click keymap, and the per-tab status dot with the
same color semantics.

**Adapt.** Two changes for our backend: (1) the `status` payload should come from the
**backend's synced PR record over SSE**, not be pushed up from client-side CI
polling — the tab dot updates live when the daemon re-syncs, with zero client fetch.
(2) Persisting *which tabs are open* is fine in localStorage, but consider letting
the backend remember open-tab sets per user later (out of scope for CCT-605). Add a
`draft`/`merged`/`closed` enum that matches our `/v1` PR state contract (CCT-604)
rather than pulldash's ad-hoc strings.

**Skip.** Nothing here is wasteful. The `try/catch` around `useTabContext` in the PR
content (to tolerate being rendered outside a provider) is a smell we don't need if
our component tree is disciplined.

---

## 2. Diff parse + highlight pipeline (→ CCT-608 canvas diff, CCT-605)

**What pulldash does.** This is the crown jewel and the most reusable idea. A
**Web-Worker pool** (`src/browser/lib/diff.ts`) sized to
`max(navigator.hardwareConcurrency, 4)` with **no upper cap**, round-robin
dispatched. Each worker (`diff-worker.ts`, ~850 lines) does, entirely off the main
thread:

- **Parse** the unified-diff patch via `gitdiff-parser` (it synthesizes a fake
  `diff --git` header around GitHub's bare patch so the parser accepts it).
- **Modified-line pairing**: a real algorithm, not a heuristic. It pairs delete/insert
  changes within a line-distance window (`maxDiffDistance: 30`) whose word-diff change
  ratio is under a threshold (`maxChangeRatio: 0.45`), using `Int32Array` pairing
  tables and a prefix-sum of unpaired deletes to decide whether a candidate pair is
  "crossed" by an intervening unpaired delete (and should be left unpaired). Paired
  lines render as a single **modified** row with inline intra-line segments.
- **Inline intra-line diff**: `diffWords`, then for adjacent delete→insert word pairs
  it upgrades to `diffChars` *only if* the edit is under `inlineMaxCharEdits: 4`
  characters (keeps tiny typo-fixes char-precise, avoids char-soup on big rewrites).
- **Syntax highlighting** via `refractor` (Prism). Crucially, it highlights the
  **whole old file and whole new file once**, line by line, carrying open Prism tags
  across line boundaries (a manual tag open/close stack) so multi-line constructs
  (block comments, template strings) highlight correctly — then indexes per-line HTML
  by line number. Individual diff lines look up their pre-highlighted HTML instead of
  re-highlighting a fragment out of context. Falls back to per-fragment highlight
  when full file content isn't available.
- **Skip blocks**: gaps between hunks become explicit `{ type: "skip", count,
  content }` rows (with the hunk `@@` context header text), so "N unchanged lines"
  is a first-class, expandable row.

The worker also supports on-demand `highlight-lines` for expanding skip blocks. The
main-thread service wraps postMessage in promises keyed by request id, plus an
optional LRU-ish `Map` cache (cap 500, evict-oldest-100) keyed by a cheap string hash.

**Why it works.** Parsing + highlighting a 5000-line diff is tens of ms of pure CPU;
doing it in a worker keeps scroll and keyboard nav at 60fps. Highlighting whole files
once and indexing by line number is both faster (one Prism pass per file vs. one per
line) and *correct* for multi-line tokens — a subtle bug most diff viewers get wrong.
The pairing algorithm is what makes "modified line with inline word diff" look good.

**Adopt.** The worker-pool architecture, the parse/pair/highlight split, the
**whole-file-highlight-then-index-by-line** technique, the modified-line pairing
approach (distance window + change-ratio + crossed-delete check), the char-vs-word
inline threshold, and first-class skip-block rows. This is behavior we want
line-for-line identical in *feel*, reimplemented from scratch.

**Adapt — this is the big backend divergence.** pulldash fetches full old/new file
blobs from GitHub *per file* on the client (`useDiffLoader` does two `getFileContent`
calls) purely so it can do context-correct highlighting, then aggressively prefetches
±diffs. **Our backend already has the diffs (and can have the blobs) warm in
Postgres.** Options, in preference order: (a) backend ships the patch plus enough
file context for correct highlighting over `/v1`, worker only parses+highlights; or
(b) backend ships *already-parsed, already-highlighted* line HTML and the worker pool
mostly disappears. Given CCT-608 wants a **Canvas 2D diff pane** for huge diffs, the
worker's *output shape* (per-line: type, old/new line number, array of highlighted
segments) is exactly the display list a canvas renderer consumes — so keep the
worker's data model even if the renderer becomes canvas instead of DOM rows.

**Skip.** The client-side `getFileContent` fetch-per-file and the ±5/−2 prefetch
window (`useDiffLoader`) — the backend makes both unnecessary. The main-thread
`parseDiffCached`/`diffCache` string-hash cache and the near-duplicate synchronous
`src/api/diff.ts` (a second, non-worker copy of the whole pipeline, apparently dead/
legacy) are both cruft born of not having a backend.

---

## 3. Virtualized rendering (→ CCT-608, CCT-605)

**What pulldash does.** The diff view flattens hunks + skip blocks into a single row
list and renders it with `@tanstack/react-virtual` (`useVirtualizer`,
`measureElement` for dynamic heights, **`overscan: 100`** — deliberately huge so
keyboard nav lands on already-rendered rows and `scrollToIndex` is instant). Split
(side-by-side) vs. unified is a user preference persisted to localStorage; split mode
converts the line list into left/right pairs. Every row component
(`VirtualRowRenderer`, `DiffLineRow`, `SplitDiffLineRow`, `SkipBlockRow`) is `memo`'d,
lines use `contain-layout`, and content is injected as pre-highlighted HTML strings.
The **command palette** and **file list** are independently virtualized too.

**Pre-computed navigation index.** When a diff loads, the store walks every rendered
row once and builds an array of `{ lineNum, rowIndex, … }` navigable items. Arrow-key
navigation is then an O(1) array step + `scrollToIndex`, never a DOM query or scan.
The README calls this out explicitly; it's the difference between snappy and janky
`j`/`k`.

**Why it works.** High overscan + memoized rows + external-store state (§5) means
focusing line 5000 doesn't re-render the file tree or re-highlight anything; it
updates one data attribute and scrolls. The pre-built nav index removes per-keystroke
DOM work.

**Adopt.** Virtualize the diff, the file list, and the command palette. Pre-compute
the navigable-line index on diff load. Persist unified/split preference. Keep rows
memoized and inject pre-highlighted HTML.

**Adapt.** For CCT-608's canvas pane, "virtualization" becomes "only paint visible
rows onto the canvas" — same idea, different primitive — and the pre-computed nav
index becomes the canvas's row→y-offset map. The huge-overscan trick is DOM-specific;
canvas gets the same smoothness from cheap repaint.

**Skip.** Nothing; this section is close to pure gain.

---

## 4. Bookmarklet + route mirroring (→ CCT-605)

**What pulldash does.** The headline growth feature: **"replace `github.com` with
`pulldash.com` in any PR URL."** Because the review route is
`/:owner/:repo/pull/:number` — identical path shape to GitHub — a github.com PR URL
maps to the same path on their host by swapping the host. A `BookmarkletDialog`
offers a draggable `javascript:` bookmarklet that regex-matches the current
github.com PR URL (`/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/`) and redirects to
`<origin>/owner/repo/pull/number`. There's also a top-bar **"PR URL…"** text input
that accepts a pasted github.com URL and opens the tab via the same regex. The
bookmarklet dialog is dismissable and remembers dismissal in localStorage; it's
hidden inside Electron. (Building the `javascript:` anchor needs
`dangerouslySetInnerHTML` to dodge React's URL sanitizer — a hack, but contained.)

**Why it works.** Zero-friction entry from where reviewers already live (a GitHub PR
page). Mirroring GitHub's exact path shape means muscle memory and shared links Just
Work.

**Adopt.** Mirror GitHub's `/:owner/:repo/pull/:number` path shape on our host. Ship
the host-swap bookmarklet and the paste-a-URL input. Dismissable, remembered,
hidden in any desktop/embedded context.

**Adapt.** Our host is behind the cctui plugin/deploy (CCT-610/612), so the
bookmarklet origin must be generated from `window.location.origin` at runtime (as
pulldash does) rather than hard-coded. Consider also accepting GitHub *notification*
and *files* URLs, since our backend syncs notifications (CCT-602).

**Skip.** Nothing structural; just re-implement the trivial regex and origin-swap
ourselves.

---

## 5. External store + review-flow UX (→ CCT-605, CCT-609)

**External store (`useSyncExternalStore`).** All PR-review state lives in a plain
class store outside React — `subscribe`/`getSnapshot`, listeners fire on `set()`.
Components subscribe via selector hooks (`usePRReviewSelector`) so focusing a line
updates one subscriber, not the tree. `viewedFiles`, `selectedFiles`,
`loadingFiles`, `expandingSkipBlocks` are `Set`s; `commentRangeLookup` is a
`Record<string, Set<number>>`. **Viewed state, pending comments, review body, and
unified/split preference all persist to localStorage** under per-PR keys
(`<storageKey>-viewed`, `-pending`, `-body`). Hash navigation (`useHashNavigation`)
mirrors the focused file/line into `location.hash` — **pushState on file change**
(back/forward jumps files), **replaceState on line change** (no history spam) — and
subscribes to the store *directly* (not via React) to update the hash without causing
re-renders. Deep links restore file+line on load.

**Keyboard map (`useKeyboardNavigation`).** The full review keymap, ignoring events
from inputs/textareas/contenteditable: `↑/↓` line, `Ctrl+↑/↓` jump 10, `←/→` switch
side in split, `Shift+arrow` extend selection, `j`/`k` prev/next **unviewed** file
(wrapped in `startTransition` so React can interrupt during rapid nav), `v` toggle
viewed (multi if files selected), `g` goto-line mode (digit entry, `Tab` toggles
side, `Enter` jumps), `o` overview, `c` comment on focused line, `e` edit, `r` reply,
`d` delete (with permission check + confirm), `s` open submit-review, `Esc`
cancel/clear, `Enter` expand focused skip block. Permission gating: `ADMIN`/`MAINTAIN`
edit/delete any comment, `WRITE` only own. Some actions dispatch `CustomEvent`s
(`pr-review:delete-comment`, `pr-review:open-submit-review`, `…:expand-skip-block`)
so the keymap stays decoupled from the components that own the DOM/API call.

**Review flow.** Pending comments are **optimistic**: added to local state instantly
for feedback, then synced to GitHub's *pending review* via GraphQL, and the local
comment is back-patched with the returned node/database ids. On load,
`usePendingReviewLoader` pulls any existing GitHub pending review and hydrates local
pending comments (so a review-in-progress survives reload / appears cross-device).
Submit (`useReviewActions`) prefers submitting the existing GraphQL pending review
node with `APPROVE`/`REQUEST_CHANGES`/`COMMENT` + body; falls back to a REST
`createReview` with all comments if there's no pending-review node. After submit it
invalidates the timeline cache, refetches comments/reviews/timeline, navigates to the
overview, and scrolls to the freshly created review. Thread resolve/unresolve
(`useThreadActions`) calls GraphQL then optimistically flips `is_resolved` on every
comment sharing the thread id.

**Why it works.** The external store is the backbone of the performance story:
line-level focus changes don't re-render unrelated UI. Optimistic local-first writes
make commenting feel instant. Persisting pending review + viewed state to localStorage
means an interrupted review isn't lost.

**Adopt.** The external-store-with-selectors architecture, the full keyboard map and
its input-guarding, `startTransition` around rapid file nav, the goto-line mode, the
CustomEvent decoupling for keymap→action, optimistic comment/thread writes, the
hash-nav pushState-vs-replaceState distinction and deep-link restore, and the
permission-gated edit/delete.

**Adapt.** (1) **Pending review + viewed state should live server-side**, not
localStorage — our backend can persist review-in-progress and viewed state per user
per PR, so it survives across devices/browsers, not just reloads. localStorage becomes
an offline/optimistic cache, not the source of truth. This directly feeds **CCT-609
multifold viewed-state** (mark a *tree node* viewed → collapse everything under it):
pulldash only has per-file viewed booleans in a `Set`; we extend that to hierarchical
tree-node viewed state, ideally server-synced. (2) After submit, we don't
invalidate+refetch — the backend pushes the new review over SSE and the store applies
it. (3) Thread resolve/optimistic flip stays, but reconcile against SSE truth.

**Skip.** The manual GitHub GraphQL/REST dance for pending reviews, replies, resolve,
edit/delete, and the "find the review I just created by sorting `submitted_at`"
scroll-target hack — all of that is because pulldash talks straight to GitHub. Our
`/v1` contract (CCT-604) gives us a typed mutation that returns the created review id
directly.

---

## 6. Command palette + fuzzy file search (→ CCT-605)

**What pulldash does.** `Cmd/Ctrl+K` (or `Ctrl+P`) toggles a `cmdk`-based palette,
bound in the capture phase to beat the browser. It fuzzy-searches the changed-file
list with a hand-rolled scorer: exact basename > basename-without-ext > prefix >
word-boundary substring (bonus after `. - _`) > full-path substring > subsequence
fuzzy (with a consecutive-char bonus), each tier length-normalized. Results are
virtualized; the query is a `useDeferredValue` so typing stays responsive and a stale
indicator (yellow search icon) shows while the deferred value lags. Matched
characters are highlighted in results; viewed files are dimmed and badged; add/delete
counts render inline. Palette closes on route change.

**Why it works.** Fuzzy-finding across hundreds of changed files is the fastest way to
jump around a big PR; the tiered scorer feels like an editor's file finder. Deferred
value + virtualization keeps it smooth on huge file lists.

**Adopt.** The palette, the tiered fuzzy scorer, virtualized results,
`useDeferredValue` + stale indicator, match highlighting, viewed-dimming, inline
add/del counts, capture-phase binding, close-on-route-change.

**Adapt.** Extend the palette beyond file jump into a real **command** palette (open
PR by URL, switch tab, toggle split/unified, submit review, jump to next unviewed) —
pulldash only does files. Pull the file list from store state that the backend
already warmed. Later, palette can search *across PRs* the backend has synced, not
just files in the current one.

**Skip.** Nothing; scorer and palette are cheap and self-contained.

---

## 7. Request-cache / SWR patterns — the catalogue of what the backend deletes (→ CCT-601/604)

**What pulldash does** (all in `src/browser/contexts/github.tsx`, ~3000 lines). This
is the machinery a no-backend app *must* build and that our sync daemon replaces:

- **`RequestCache`**: in-memory `Map` + `localStorage` mirror (`gh_cache:` prefix,
  30s default TTL). `get` (fresh-only), **`getStale` (returns `{data, isStale}` for
  SWR — render stale instantly, revalidate in background)**, `set(persist?)`,
  `invalidate(pattern?)` (substring match), and a **pending-promise map** so
  concurrent callers for the same key share one in-flight request (dedup). Persisted
  keys survive reload for "instant UI" (e.g. current user loaded from localStorage
  before the network confirms).
- **SWR everywhere**: current user (5-min fresh TTL), PR lists (30s), checks, etc. —
  each reads stale, shows it, and only revalidates if past the fresh window, coalescing
  via the pending map.
- **`GraphQLBatcher`**: queues GraphQL queries in a 5ms window and fires them together
  (parallel, since GitHub has no true multi-query batching) to collapse bursts.
- **Octokit hook wrapping**: intercepts 401 (→ token-revoked callback) and 403 with
  `x-ratelimit-remaining: 0` (→ rate-limited callback). PR-list fetches use an
  `AbortController` to cancel superseded requests.
- **CI-status rollup**: fetches combined status + check-runs for `head.sha`, folds
  `action_required`/`in_progress`/conclusions into the single
  `pending|success|failure|none|action_required` enum used by the tab dot.
- **Device-flow auth** proxied through the two Hono endpoints; also PAT auth (the very
  commit we audited); tokens in localStorage; unauth Octokit for public repos.

**Why it exists.** With no backend, the browser is the cache, the rate-limit manager,
the request coalescer, and the poller. SWR + persisted cache is how pulldash fakes
"instant opens" — show stale, revalidate behind. The 30s TTLs, the batcher, the
abort-on-supersede, and the rate-limit hooks are all symptoms of hammering GitHub
directly from every open client.

**What we adopt.** Almost nothing structural — but **the *ideas* map onto the
backend**: our sync daemon *is* the SWR layer (ETag polling = revalidation, Postgres
= the persisted cache, SSE = the "revalidated, here's fresh data" push). The
CI-status enum rollup logic is worth porting **into the backend** so the synced PR
record carries a ready-to-render status. The 401/rate-limit handling moves
server-side (one token pool, not N clients). Keep a *thin* client cache only as an
SSE-offline fallback.

**What we skip / delete outright.** `RequestCache`, `getStale`/SWR plumbing,
`GraphQLBatcher`, per-client `localStorage` `gh_cache:` mirror, `AbortController`
supersede dance, client-side rate-limit handling, device-flow/PAT-in-localStorage,
unauth Octokit, and the ±file diff prefetch (§2). All of it is "no backend" tax. The
backend (CCT-601) polls once per account with ETags, stores JSONB, and pushes over
SSE; the `/v1` contract (CCT-604) hands clients typed, warm data. **This whole
section is the strongest evidence for the epic's thesis: the warm backend makes the
single largest and gnarliest part of pulldash unnecessary.**

---

## 8. Themes (→ CCT-607)

**What pulldash does.** One theme only: a dark UI with Tailwind v4 `@theme`
oklch tokens (`--background`, `--foreground`, …) in `index.css`, a `.dark` custom
variant wired but effectively always-on, a GitHub-dark syntax palette, and a bundled
**One Light** Prism theme in `ui/diff/theme.css` (present but the app ships dark).
Syntax colors are CSS variables consumed by the pre-highlighted HTML the worker emits.

**Why it's relevant.** The token structure — semantic CSS variables for chrome plus a
separate Prism/refractor palette for code — is exactly the seam CCT-607 needs: swap
the variable set to switch themes without touching component code, and swap the syntax
palette independently.

**Adopt.** The semantic-token + separate-syntax-palette split; CSS-variable-driven
highlighting so the worker output is theme-agnostic (it emits Prism class names, CSS
colors them).

**Adapt / extend (this is CCT-607's actual work).** pulldash has *one* theme; we ship
**four** — light, dark, colorblind-light, colorblind-dark. That means a real theme
switcher (persisted, ideally server-synced with other prefs), a diff add/remove color
scheme that stays legible under colorblind palettes (don't rely on red/green alone —
add texture/position cues), and multiple Prism palettes selected by active theme.

**Skip.** The dead One-Light stylesheet that ships unused.

---

## 9. Misc worth noting

- **`memo` discipline**: nearly every render-hot component is memoized and reads state
  via narrow selectors. Worth mirroring — it's load-bearing for the perf story.
- **`CustomEvent` bus** for keymap→component actions is a pragmatic decoupling; fine to
  adopt sparingly, but our store can also just expose the action directly.
- **Electron packaging** (electron-builder, auto-update): out of scope — we're a web
  plugin in cctui (CCT-610), not a desktop app. Skip.
- **Telemetry (PostHog)**: the infra overlay already neutralizes it via a no-op
  `telemetry.tsx` shim at build. We add none.
- **Deployment today**: the personal infra repo runs upstream pulldash at this
  pinned sha, built from source (no upstream image), telemetry stripped.
  **CCT-612 retires this overlay** once our review center supersedes it.

---

## Pattern → epic ticket map

| Pattern | Adopt / Adapt / Skip | Lands in |
|---|---|---|
| Tab system, deterministic ids, active-only mount, status dot | Adopt; status via SSE | CCT-605 |
| Worker-pool diff parse + whole-file-highlight-by-line + skip blocks | Adopt pipeline; source diffs from backend | CCT-608, CCT-605 |
| Modified-line pairing + inline word/char threshold | Adopt (reimplement) | CCT-608 |
| Virtualized diff/file/palette + pre-computed nav index | Adopt; canvas variant | CCT-608, CCT-605 |
| Bookmarklet + `/:owner/:repo/pull/:n` mirror + paste-URL input | Adopt; runtime origin | CCT-605 |
| External store + selectors + hash nav deep-link | Adopt | CCT-605 |
| Full keyboard map, goto-line, permission gating | Adopt | CCT-605 |
| Optimistic pending comments / review submit / thread resolve | Adopt UX; server-persist state, typed mutations | CCT-605, CCT-604 |
| Per-file viewed `Set` in localStorage | Adapt → hierarchical, server-synced | CCT-609 |
| Command palette + tiered fuzzy scorer + deferred value | Adopt; extend to commands + cross-PR | CCT-605 |
| RequestCache / SWR / GraphQLBatcher / rate-limit hooks / abort | **Skip** — backend replaces | CCT-601, CCT-604 |
| CI-status enum rollup | Adapt → compute server-side | CCT-601 |
| Semantic CSS tokens + separate syntax palette | Adopt seam; extend to 4 themes + colorblind | CCT-607 |
| Electron / PostHog / device-flow-in-localStorage | Skip | — |

---

## License compliance note

`coder/pulldash` is licensed **AGPL-3.0-only**. This document is an audit of observed
*behavior and UX semantics*, written in our own words; it contains no substantial
source excerpts (only trivial, non-copyrightable identifiers, route path shapes, and
config constants named for reference). The gh-review center must **reimplement** these
patterns independently — engineers should work from this catalogue and the product
behavior, **not** from pulldash's source files, to keep our implementation clean-room
and free of AGPL-derived code. Do not copy, paste, or transliterate pulldash source
into the cctui/ghreview codebase. If in doubt about a specific mechanism, describe the
desired behavior in a ticket and implement from the description.

_Audited commit: `1fd065eb179ad26e46e08194e287aac432268149` (2026-01-05)._
