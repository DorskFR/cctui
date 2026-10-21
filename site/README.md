Docs and landing site for cctui (Astro Starlight). Run `npm install && npm run dev` here; `npm run build` writes `dist/` and `.github/workflows/site.yml` deploys it to GitHub Pages under `/cctui`.
Clips in `public/media/<slug>.{mp4,webm,poster.webp}` are produced by the recording pipeline (slugs listed in `src/lib/clips.ts`); the site renders a placeholder box while a clip is missing.
Screenshots in `public/shots/` are copied from `docs/journeys/*/desktop-dark/`.
