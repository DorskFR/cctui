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
  windowing math; `split.ts` derives the side-by-side row model from the same
  unified model; `highlight.ts` wraps a slim highlight.js core (per-extension
  language map) behind a memoizing per-line cache. `components/DiffView.svelte` is
  the single renderer: virtualized DOM rows, unified or split.
- **Viewed state** (`src/lib/diff/tree.ts`, `collapse.ts`, `api/viewed.ts`) —
  **CCT-609**. `buildFileTree()` turns the flat file list into a nested,
  single-child-compressed directory tree; `FileTree.svelte` renders per-file and
  per-folder checkboxes with `n/m` progress (a folder toggle cascades to every file
  beneath). Marking a file viewed collapses it via `collapseViewedFiles()`, a pure
  row-model transform that drops a viewed file's body
  rows and leaves a "viewed — N lines hidden" header stub (clicking the file in the
  tree peek-expands it). State comes from `GET …/pulls/{n}/viewed` via tanstack query
  and live `pr.viewed_state.updated` SSE; toggles are optimistic
  (`applyOptimisticViewed`) with rollback on error. `tree.ts`, `collapse.ts` and
  `viewed.ts` are pure; `tree.ts` and `viewed.ts` have their own test files, and
  `collapse.ts` is covered by the `collapseViewedFiles` cases in `tree.test.ts`.

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
  `variable,type,punctuation}` per theme, mapped onto highlight.js token classes in
  `src/lib/markdown/hljs-tokens.css`.

**Tokens contract.** `DiffView.svelte` styles itself with these CSS variables
directly and hardcodes no colors; adding a color means adding a token, not a literal.

WCAG AA (≥4.5:1) is enforced by `src/lib/theme/contrast.test.ts`, which parses the
`data-theme` blocks out of `tokens.css` and asserts text and diff fg/bg pairs across
all four themes (hex→luminance helper in `contrast.ts`).

## Keyboard map

Implemented (input/textarea-guarded):

- `j` / `k` — next / previous **hunk**
- `J` / `K` — next / previous **file**
- `g` then `d` — jump to the diff (first hunk)
- `Cmd/Ctrl+1..9` — select tab _n_ · `Cmd/Ctrl+W` — close current tab ·
  middle-click / `×` — close tab
- `v` — toggle **viewed** on the file under the cursor
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
  yields a small visible slice in O(1) — that single `computeWindow` assertion is
  the only coverage `virtual.ts` has. Rows render fixed-height and virtualized, so
  only the visible window plus overscan is ever in the DOM.
- **Highlighting is memoized, not repeated per frame.** `highlight.ts` exposes
  `highlightLineCached` — a bounded `Map` keyed by `(lang, line content)`. A row is
  highlighted the first time it scrolls into the window and is a cache hit forever
  after, so scrolling back and forth costs map lookups instead of `hljs.highlight()`
  passes. `highlight.test.ts` pins this with a spy on the underlying highlighter.

No frame-rate number is claimed here: nothing in this repo measures paint or frame
time, and jsdom/happy-dom cannot. Treat the tests above as algorithmic-cost
tripwires and profile in a real browser if you need frame numbers.

## Landed since the foundation

- **CCT-607** — four themes + syntax palettes.
- **CCT-609** — hierarchical, server-synced viewed state.
- **CCT-610** — embeds into cctui-ui with injected cctui bearer auth (see
  _Standalone vs embedded_).
- Per-line syntax highlighting, side-by-side (split) mode, inline comment drafts
  and review submit/publish.

## Deferred

- The command-palette UI + fuzzy file finder (the `Cmd/Ctrl+K` keybind resolves to
  an `openPalette` action; nothing renders it yet).
- Modified-line pairing with inline word/character diff.
- Server-remembered open-tab sets.

## Regenerating the API types

`src/generated/api.ts` is a verbatim copy of the backend's generated client
(`ghreview/src/generated/api.ts`). After a `/v1` contract change, refresh it:

```sh
cp ../ghreview/src/generated/api.ts src/generated/api.ts
```
