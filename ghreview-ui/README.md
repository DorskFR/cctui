# ghreview-ui

Frontend for the cctui GitHub review center (epic **CCT-600**). Foundation
delivered in **CCT-605**: a tabbed, keyboard-first PR review UI that opens
instantly from the warm `/v1` backend (`ghreview/`) — **zero GitHub round trips
in the open path**.

It is a **Svelte 5 + Vite** single-page app, built to run standalone against the
backend today and to mount into `cctui-ui` in **CCT-610**. It shares webui's
tooling (Svelte 5 runes, `@tanstack/svelte-query`, biome, vitest) and owns its own
minimal CSS with design tokens (every color is a CSS custom property in
`src/app.css`; **CCT-607** formalizes themes and syntax palettes).

## Run standalone against the backend

```sh
mise use bun@latest        # or use the repo's mise shims
bun install
GHREVIEW_URL=http://localhost:8790 bun run dev   # vite dev server on :5290
```

The dev server proxies `/v1` (including `/v1/events` SSE) to `GHREVIEW_URL`
(default `http://localhost:8790`), so the app is same-origin in dev. For a hosted
build, set `VITE_GHREVIEW_URL` to the backend origin at build time instead.

### Auth stub

Auth reuses cctui bearer tokens (see `ghreview/README.md`). Until **CCT-610**
wires real cctui auth, the app prompts for a token on first load and stores it in
`localStorage` (`ghreview:token`) alongside an optional default account
(`ghreview:account`). `VITE_GHREVIEW_TOKEN` / `VITE_GHREVIEW_ACCOUNT` seed these
for local dev. The token is sent as `Authorization: Bearer …` on every `/v1` call
and as `?access_token=` on the SSE stream.

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
  (`pr.updated` / `notification.*` / `sync.status`) into tanstack cache
  invalidations. **No polling in the UI.** `sseActions()` is a pure map that is
  unit-tested; the EventSource wiring is a thin shell around it.
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
  (`components/DiffView.svelte`) is registered today, and **CCT-608**'s canvas pane
  registers a `kind: "canvas"` renderer behind the same interface.

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

- **CCT-607** — theme system (4 themes incl. colorblind) + syntax palettes. Tokens
  are already CSS custom properties, and `data-theme` switching is stubbed.
- **CCT-608** — Canvas 2D diff pane behind the existing `DiffRenderer` seam;
  whole-file syntax highlighting; modified-line pairing + inline word/char diff.
- **CCT-609** — hierarchical, server-synced viewed state.
- **CCT-610** — mount into cctui-ui; real cctui bearer auth (replaces the token
  prompt); server-remembered open-tab sets.
- Also deferred here: the command-palette UI + fuzzy file finder (keybind is
  wired), review submit / optimistic comments, side-by-side (split) diff view.

## Regenerating the API types

`src/generated/api.ts` is a verbatim copy of the backend's generated client
(`ghreview/src/generated/api.ts`). After a `/v1` contract change, refresh it:

```sh
cp ../ghreview/src/generated/api.ts src/generated/api.ts
```
