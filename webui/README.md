# cctui-webui

Mobile-first standalone SPA for cctui — Svelte 5 + SvelteKit (`adapter-static`,
app mode, no SSR) + TanStack Query. Replaces the legacy single-file
`crates/cctui-server/web/index.html` (kept for side-by-side comparison).

## Layout

- `src/lib/styles/variables.css` — **the theme**. Every color/space/font/radius
  is a token here; swapping the theme = replacing this one file. `reset` + base
  + components live in `app.css` and only consume tokens.
- `src/lib/bindings/` — TypeScript types **generated from the Rust structs** via
  ts-rs (committed). Regenerate with `npm run bindings` (or `make bindings` at
  the repo root) after changing an annotated Rust type. Imported via `@bindings`.
- `src/lib/api.ts` / `queries.ts` / `ws.svelte.ts` — typed fetch client, thin
  TanStack query/action wrappers, and the shared TUI websocket.
- `src/routes/` — `/` overview, `/sessions` (list + conversation drawer + spawn),
  `/users` (machines + tokens admin).

## Dev

```sh
npm install
npm run dev        # http://localhost:5273
npm run check      # svelte-check
npm run build      # static SPA into build/
```

The API origin is runtime config: `static/config.js` sets
`window.CCTUI_CONFIG.apiBase`. Override that file per-deployment (it is served
with `no-store`) to retarget the API without rebuilding. Auth is a Bearer token
held in `localStorage`; the server allows the cross-origin calls via CORS.

## Deploy

Built into an nginx image (`webui/Dockerfile`) and shipped as the standalone
`cctui-ui` Deployment (wire it up in your own GitOps/kustomize overlay).
`make ui/image/release` from the repo root builds + pushes the image.
