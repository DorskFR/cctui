# ghreview-ui

Frontend for the cctui GitHub review center (epic **CCT-600**). Foundation
delivered in **CCT-605**: a tabbed, keyboard-first PR review UI that opens
instantly from the warm `/v1` backend (`ghreview/`) — **zero GitHub round trips
in the open path**.

It is a **Svelte 5 + Vite** single-page app that runs **standalone** against the
backend and **embeds** into `cctui-ui` (the connector, **CCT-610**). It shares
webui's tooling (Svelte 5 runes, `@tanstack/svelte-query`, biome, vitest) and owns
its own minimal CSS with design tokens (every color is a CSS custom property in
`src/tokens.css`; **CCT-607** formalized four themes and syntax palettes).

## Standalone vs embedded

The same code runs two ways, selected by whether an embedder injects a runtime
config (`configureRuntime()` in `src/lib/api/config.ts`):

- **Standalone** — `src/main.ts` mounts `App.svelte`, which shows the token
  `AuthGate` and reads backend URL / token / account from `localStorage` +
  `VITE_*` env. `main.ts` imports `src/app.css` (document-level base rules).
- **Embedded** — `cctui-ui` imports `src/Review.svelte` and passes
  `{ baseUrl, token, account?, basePath }` as props. `Review` injects them via
  `configureRuntime()` (so the API client + SSE use the host's URL + bearer, no
  login stub), mounts the shared `Shell.svelte`, and imports `src/embed.css` —
  which reuses `src/tokens.css` but scopes the base rules under `.ghreview-embed`
  so they never leak onto the host's `<body>`. The theme lives on the embed
  container (not `<html>`), so switching it never touches the cctui chrome. The
  router runs under `basePath` (`/review`) while the GitHub-mirrored paths stay
  intact.

`src/main.ts` and `App.svelte` keep their standalone behaviour unchanged.

## Run standalone against the backend

```sh
mise use bun@latest        # or use the repo's mise shims
bun install
GHREVIEW_URL=http://localhost:8790 bun run dev   # vite dev server on :5290
```

The dev server proxies `/v1` (including `/v1/events` SSE) to `GHREVIEW_URL`
(default `http://localhost:8790`), so the app is same-origin in dev. For a hosted
build, set `VITE_GHREVIEW_URL` to the backend origin at build time instead.

### Auth

Auth reuses cctui bearer tokens (see `ghreview/README.md`); the token is sent as
`Authorization: Bearer …` on every `/v1` call and as `?access_token=` on the SSE
stream.

- **Standalone** — the `AuthGate` prompts for a token on first load and stores it
  in `localStorage` (`ghreview:token`) with an optional default account
  (`ghreview:account`); `VITE_GHREVIEW_TOKEN` / `VITE_GHREVIEW_ACCOUNT` seed these
  for local dev.
- **Embedded** — cctui-ui injects a bearer minted for the signed-in user (CCT-603
  resolves it against the shared DB), so there is no second login.

## Commands

```sh
bun run dev         # vite dev server (proxies /v1 to GHREVIEW_URL)
bun run build       # production build to dist/
bun run check       # typecheck (svelte-check) + lint (biome) + tests (vitest) — the gate
bun run test        # vitest only
```

## Architecture

- **Data layer** (`src/lib/api/`) — generated `/v1` types (`src/generated/api.ts`,
  copied verbatim from `ghreview/src/generated/api.ts`), a thin bearer-token
  `fetch` wrapper (`client.ts`), tanstack-query keys/options (`queries.ts`), and
  GitHub-payload narrowing (`types.ts`). Envelopes' `payload` is narrowed with
  hand-written minimal GitHub-shaped interfaces (no octokit dependency pulled in).
- **SSE** (`api/sse.ts`) — subscribes to `/v1/events` and turns each change hint
  (`pr.updated` / `pr.viewed_state.updated` / `notification.*` / `sync.status`) into
  tanstack cache invalidations. **No polling in the UI.** `sseActions()` is a pure
  map that is unit-tested; the EventSource wiring is a thin shell around it.
- **Router** (`src/lib/router/`) — a tiny history router. Routes mirror GitHub:
  `/` (PR list + filters), `/inbox` (notifications), `/bookmarklet`, and
  `/:owner/:repo/pull/:number` (PR view). `parseRoute()` is pure/tested.
- **Tabs** (`src/lib/stores/`) — deterministic PR-coordinate ids
  (`pr-<owner>-<repo>-<number>`), idempotent open, adjacent-selection close,
  localStorage restore with validation/fallback. All reducer logic lives in
  `tabs-core.ts` (pure, fully unit-tested); `tabs.svelte.ts` is the `$state` +
  persistence wrapper. Per-tab status dot (pr/ci/mergeable) is driven from synced
  PR data and refreshed live via SSE.
- **Diff** (`src/lib/diff/`) — `parse.ts` turns each file's unified `patch` (from
  the stored GitHub files payload) into a flat row model with correct old/new line
  numbers and first-class file/hunk rows; `navindex.ts` pre-computes the
  file/hunk navigation index (O(1) j/k stepping); `virtual.ts` is the fixed-row
  windowing math; `renderer.ts` is the **`DiffRenderer` seam** — the DOM renderer
  (`components/DiffView.svelte`) and **CCT-608**'s canvas pane
  (`components/CanvasDiffView.svelte`) both register behind the same interface and
  are swappable at runtime (see **Canvas diff pane** below).
- **Viewed state** (`src/lib/diff/tree.ts`, `collapse.ts`, `api/viewed.ts`) —
  **CCT-609**. `buildFileTree()` turns the flat file list into a nested,
  single-child-compressed directory tree; `FileTree.svelte` renders per-file and
  per-folder checkboxes with `n/m` progress (a folder toggle cascades to every file
  beneath). Marking a file viewed collapses it in **both** renderers via
  `collapseViewedFiles()`, a pure row-model transform that drops a viewed file's body
  rows and leaves a "viewed — N lines hidden" header stub (clicking the file in the
  tree peek-expands it). State comes from `GET …/pulls/{n}/viewed` via tanstack query
  and live `pr.viewed_state.updated` SSE; toggles are optimistic
  (`applyOptimisticViewed`) with rollback on error. `tree.ts`, `collapse.ts` and
  `viewed.ts` are pure and fully unit-tested.

## Themes (CCT-607)

Four first-class themes, selected by a `data-theme` attribute — on `<html>`
standalone, or on the `.ghreview-embed` container when embedded (so the host's
theme is untouched): **dark** (default, matches cctui), **light**,
**colorblind-dark**, **colorblind-light**. `src/lib/theme/theme.ts` owns
selection: the resolved theme is an explicit `localStorage` choice
(`ghreview:theme`), else `prefers-color-scheme`, else dark. `initTheme()` applies
it in `main.ts` before mount (no flash); the top-bar `<select>` persists changes
via `setTheme()` standalone or the embed theme context.

All colors are CSS custom properties in `src/tokens.css` (shared by both the
standalone `src/app.css` and the embedded `src/embed.css`), grouped in semantic
tiers:

- **Chrome** — surface (`--gh-bg`, `--gh-bg-elev`, `--gh-bg-inset`), text
  (`--gh-fg`, `--gh-fg-muted`, `--gh-fg-subtle`), border, accent, status.
- **Diff** — `--gh-diff-{add,del,context}-{bg,fg}`, `--gh-diff-gutter-{bg,fg}`,
  `--gh-diff-hunk-{bg,fg}`, plus non-color encoding: `--gh-diff-{add,del}-edge`
  (left bar) and `--gh-diff-{add,del}-glyph` (the always-rendered `+`/`−` gutter
  markers). Colorblind themes swap red/green for a blue/orange (deuteranopia/
  protanopia-safe) palette; the edge bar + glyph mean add/remove never rely on hue.
- **Syntax** — an 8-color scale `--gh-syn-{keyword,string,number,comment,function,`
  `variable,type,punctuation}` per theme, stable hooks for the deferred highlighter.

**Tokens contract for renderers.** The DOM renderer (`DiffView.svelte`) styles
itself with these CSS variables directly and hardcodes no colors. CCT-608's canvas
renderer cannot reference CSS variables, so it reads identical values through
`themeTokens()` (`src/lib/theme/theme.ts`) — a computed-style accessor returning the
diff + syntax tokens as concrete color strings. Both renderers therefore draw from
one source of truth; adding a color means adding a token, not a literal.

WCAG AA (≥4.5:1) is enforced by `src/lib/theme/contrast.test.ts`, which parses the
`data-theme` blocks out of `tokens.css` and asserts text and diff fg/bg pairs across
all four themes (hex→luminance helper in `contrast.ts`).

## Canvas diff pane (CCT-608)

A second diff renderer paints the diff surface onto a **Canvas 2D** context instead
of DOM rows, for locked-60fps fling-scroll on 10k+ line diffs. It registers behind
the same `DiffRenderer` seam as the DOM baseline, so it is a swap, not a fork.

**Toggle.** The PR header has a **DOM / Canvas** switch. The choice persists to
`localStorage` (`ghreview:renderer`) and **DOM stays the default** until canvas has
proven itself in the field. Switching remounts the diff (`{#key}`); scroll position
and the nav index are interchangeable because both renderers key off the same row
height (`ROW_HEIGHT` in `src/lib/diff/canvas/layout.ts`, imported by `DiffView.svelte`).

**Architecture** (`src/lib/diff/canvas/`):

- `layout.ts` — the shared geometry: row height, gutter/marker/code column x-bounds,
  `rowTop`/`rowAtY`, `hitTest` (pixel → row + region + file/hunk), `anchorScreenY`
  (overlay anchor that tracks scroll and zoom), and scroll clamp/reveal math. This is
  the single source of truth the DOM renderer, canvas paint, hit-testing, and the
  comment overlay all read, so positions never drift between them.
- `paint.ts` — a **pure** `paint(ctx, params)` that draws only the virtualized window
  (visible rows + overscan) from the shared flat row model. It is
  devicePixelRatio-aware (`setTransform(dpr,…)`) and reads every color from
  `themeTokens()` (CCT-607) — no hardcoded colors. `ctx` is typed as a structural
  subset (`Ctx2D`) satisfied by both `CanvasRenderingContext2D` and
  `OffscreenCanvasRenderingContext2D`, which is what lets the same draw code run on the
  main thread and in the worker.
- `paint.worker.ts` — receives an `OffscreenCanvas` via `transferControlToOffscreen()`
  and calls the same `paint()` off the main thread. `CanvasDiffView.svelte` uses the
  worker where supported and **falls back to a main-thread 2D context on Safari** (no
  OffscreenCanvas), transparently.
- `selection.ts` — line + line-range selection math and the `SelectionEvent` emitted
  through the `DiffRenderer` seam (`onSelectRange`), plus `rangeToClipboardText`.

**Interactions.** Wheel/pointer/touch scroll with rAF-driven momentum (fling); drag on
the gutter selects a line range and opens a minimal DOM-overlay **comment draft** widget
anchored to the range (the full review-submit UX is a later ticket; the overlay seam is
in place). Click focuses a row (same `onFocusRow` the DOM renderer emits). Keyboard nav
from `PrView` scrolls the canvas via the shared reveal math.

**Text selection trade-off.** Canvas has no native text selection. Range selection +
**range copy** is provided instead (`Cmd/Ctrl+C` with a range selected, or the draft
widget's *Copy lines*), reconstructing unified-diff prefixes. Character-level in-canvas
selection is intentionally out of scope; the DOM renderer remains available for anyone
who needs OS-native selection.

**Comment overlay.** Comment widgets live in a DOM layer above the canvas, positioned
by `anchorScreenY(rowIndex, scrollTop, rowHeight)`. Because that is pure row-offset math,
anchors stay glued to their line across scroll and zoom (row-height change).

**Measured performance.** `src/lib/diff/canvas/paint.test.ts` runs `paint()` against a
synthetic **10k-line** model through a stubbed 2D context that counts draw ops. It
asserts (a) op count is bounded by the visible window (~120 rows), **not** total rows;
(b) a 500-line and a 10k-line model issue the *identical* op count (virtualization
proof); and (c) exactly one dpr transform per frame. A wall-clock assert (`< 8ms`) guards
regressions, but **jsdom/happy-dom draw-call timing is not a real frame budget** —
treat it only as a relative regression tripwire. For real numbers, profile in a browser:
open a 10k-line PR, switch to Canvas, and record a Performance trace while fling-scrolling
— target is a locked 60fps (≤16.6ms/frame, paint typically ≪8ms).

**Limitations.** No syntax highlighting yet (deferred with the DOM renderer); no
side-by-side/split mode; in-canvas character selection is out of scope (see above);
the comment draft is a seam, not the full submit flow (later ticket).

## Keyboard map

Implemented (input/textarea-guarded):

- `j` / `k` — next / previous **hunk**
- `J` / `K` — next / previous **file**
- `g` then `d` — jump to the diff (first hunk)
- `Cmd/Ctrl+1..9` — select tab _n_ · `Cmd/Ctrl+W` — close current tab ·
  middle-click / `×` — close tab
- `Cmd/Ctrl+K` — open command palette (action wired; palette UI is **deferred**)

## Performance — the <100ms open target

Requirement #1 is instant opens. How it's met:

- **Open reads from cache, not the network.** Opening a PR tab renders
  synchronously from the warm tanstack cache: `PrView`'s query seeds `initialData`
  from `queryClient.getQueryData(["pull", …])`, so if the PR list (or a prior open)
  already warmed the record, the header + diff paint on the first frame with zero
  awaits. A background refetch reconciles, and SSE pushes later updates.
- **The parse/index path is cheap and virtualized.** `src/lib/diff/perf.test.ts`
  parses + nav-indexes a **50-file, ~10k-line** working set and asserts it
  completes in **< 100ms** (typically a few ms), and that windowing a 10k-row diff
  yields a small visible slice in O(1). Rows render fixed-height and virtualized,
  so scroll/keyboard nav stay smooth regardless of diff size.

Syntax highlighting is **deferred** (correctness + speed first); the row model
already carries per-line content ready for a later highlight pass (whole-file
highlight-then-index-by-line, per the pulldash audit) without changing the
renderer interface.

## Deferred to later tickets

- **CCT-608** — Canvas 2D diff pane behind the existing `DiffRenderer` seam is
  **landed** (see above). Still deferred on top of it: whole-file syntax highlighting,
  modified-line pairing + inline word/char diff, and the full comment-submit flow.
- **CCT-609** — hierarchical, server-synced viewed state.
- **CCT-610** — **landed**: mounts into cctui-ui with injected cctui bearer auth
  (see _Standalone vs embedded_ above). Still deferred: server-remembered
  open-tab sets.
- Also deferred here: the command-palette UI + fuzzy file finder (keybind is
  wired), review submit / optimistic comments, side-by-side (split) diff view.

## Regenerating the API types

`src/generated/api.ts` is a verbatim copy of the backend's generated client
(`ghreview/src/generated/api.ts`). After a `/v1` contract change, refresh it:

```sh
cp ../ghreview/src/generated/api.ts src/generated/api.ts
```
