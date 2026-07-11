import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

declare const process: { env: Record<string, string | undefined> };

const backend = process.env.GHREVIEW_URL || "http://localhost:8790";

export default defineConfig({
  plugins: [svelte()],
  resolve: {
    alias: {
      $lib: new URL("./src/lib", import.meta.url).pathname,
    },
  },
  server: {
    host: true,
    port: 5290,
    proxy: {
      "/v1": { target: backend, changeOrigin: true, ws: true, secure: false },
    },
  },
});
