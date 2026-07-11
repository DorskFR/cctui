/// <reference types="svelte" />
/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_GHREVIEW_URL?: string;
  readonly VITE_GHREVIEW_TOKEN?: string;
  readonly VITE_GHREVIEW_ACCOUNT?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
