import { paraglideVitePlugin } from '@inlang/paraglide-js';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Node global (no @types/node in this project); only used at build time.
declare const process: { env: Record<string, string | undefined> };

// Client build version, injected at image-build time via the CLIENT_VERSION
// build arg (= the workspace version; see webui/Dockerfile + `make
// ui/image/build`). Falls back to "dev" for local `npm run dev`/`build`.
const clientVersion = process.env.CLIENT_VERSION || 'dev';

export default defineConfig({
	plugins: [
		// No URL/cookie strategy: this SPA drives locale imperatively via setLocale
		// from the settings store, so the runtime must not auto-resolve from a path.
		paraglideVitePlugin({
			project: './project.inlang',
			outdir: './src/lib/paraglide',
			strategy: ['localStorage', 'preferredLanguage', 'baseLocale'],
			disableAsyncLocalStorage: true
		}),
		sveltekit()
	],
	define: {
		__CLIENT_VERSION__: JSON.stringify(clientVersion)
	},
	resolve: {
		// The embedded gh-review UI (CCT-610) is a sibling workspace; alias its
		// source so it is imported by path, not by package name — that keeps
		// svelte-check from parsing it (an ambient decl types the import instead)
		// and avoids pulling a second copy of svelte into the type program.
		alias: {
			$ghreview: new URL('../ghreview-ui/src', import.meta.url).pathname
		},
		// A single svelte (and query) runtime is mandatory: gh-review's context /
		// runes must share the host's instance or setContext/getContext break.
		dedupe: ['svelte', '@tanstack/svelte-query']
	},
	server: {
		host: true,
		port: 5273,
		proxy: process.env.CCTUI_PROXY
			? {
					'/api': { target: process.env.CCTUI_PROXY, changeOrigin: true, ws: true, secure: true }
				}
			: undefined
	}
});
