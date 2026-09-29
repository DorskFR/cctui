import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

// Duplicates webui/plugin-sdk/vite.ts on purpose: until the SDK is published,
// importing it would couple this build to the cctui checkout's layout.
const RUNTIME_PATHS: Record<string, string> = {
  svelte: "/plugin-runtime/svelte.js",
  "svelte/internal/client": "/plugin-runtime/svelte-internal-client.js",
  "svelte/internal/disclose-version": "/plugin-runtime/svelte-internal-disclose-version.js",
  "svelte/store": "/plugin-runtime/svelte-store.js",
  "@dorsk/tsumikit": "/plugin-runtime/tsumikit.js",
};

export default defineConfig({
  plugins: [svelte({ compilerOptions: { css: "injected" }, emitCss: false })],
  resolve: {
    alias: {
      $lib: new URL("./src/lib", import.meta.url).pathname,
    },
  },
  build: {
    outDir: "dist/plugin/web",
    emptyOutDir: true,
    minify: true,
    // Component styles are injected at mount, but the plain `import "*.css"`
    // sheets (tokens, embed, markdown, syntax) cannot be — they leave as one
    // asset the manifest's `styles[]` points the host at.
    cssCodeSplit: false,
    lib: { entry: "src/plugin.ts", formats: ["es"], fileName: () => "index.js" },
    rollupOptions: {
      external: (id: string) => id in RUNTIME_PATHS,
      output: { paths: RUNTIME_PATHS, assetFileNames: "index.css" },
    },
  },
});
