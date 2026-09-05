import { paraglideVitePlugin } from '@inlang/paraglide-js';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Node global (no @types/node in this project); only used at build time.
declare const process: { env: Record<string, string | undefined> };

// Client build version, injected at image-build time via the CLIENT_VERSION
// build arg (= the workspace version; see webui/Dockerfile + `make
// ui/image/build`). Falls back to "dev" for local `npm run dev`/`build`.
const clientVersion = process.env.CLIENT_VERSION || 'dev';

/** Proxy `/api` (and its WebSocket) at a deployed cctui, so `npm run dev` drives
 *  a real server. Two things have to be undone on the way through, or the dev
 *  origin can log in but never stay logged in:
 *
 *  - The upstream sits behind TLS, so its auth cookie carries `Secure`, and a
 *    browser will not store that over plain http on a LAN address. (localhost
 *    is exempt — it counts as a secure context — which is why only the LAN
 *    origins break.) Strip it; the hop to the upstream is still https.
 *  - The server's CORS/WS allowlist holds its own URL and `localhost:5173`, so
 *    a LAN origin is refused. Present the target's own origin upstream.
 */
export function devProxy(target: string | undefined) {
	if (!target) return undefined;
	const origin = new URL(target).origin;
	return {
		'/api': {
			target,
			changeOrigin: true,
			ws: true,
			secure: true,
			headers: { origin },
			cookieDomainRewrite: '',
			configure: (proxy: {
				on: (
					ev: string,
					fn: (res: { headers: Record<string, string | string[] | undefined> }) => void
				) => void;
			}) => {
				proxy.on('proxyRes', (res) => {
					const set = res.headers['set-cookie'];
					if (Array.isArray(set)) res.headers['set-cookie'] = set.map(stripSecure);
				});
			}
		}
	};
}

/** Drop `Secure` (and downgrade the `SameSite=None` that requires it). */
export function stripSecure(cookie: string): string {
	return cookie
		.replace(/;\s*Secure\b/gi, '')
		.replace(/;\s*SameSite=None\b/gi, '; SameSite=Lax');
}

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
		dedupe: ['svelte', '@tanstack/svelte-query', 'highlight.js', '@dorsk/tsumikit']
	},
	server: {
		host: true,
		port: 5273,
		proxy: devProxy(process.env.CCTUI_PROXY)
	}
});
