import { paraglideVitePlugin } from '@inlang/paraglide-js';
import { sveltekit } from '@sveltejs/kit/vite';
import { createReadStream, statSync } from 'node:fs';
import { extname, join, resolve as resolvePath, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig, type Plugin } from 'vite';

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

const MIME: Record<string, string> = {
	'.js': 'text/javascript',
	'.mjs': 'text/javascript',
	'.css': 'text/css',
	'.html': 'text/html;charset=utf-8',
	'.json': 'application/json',
	'.svg': 'image/svg+xml',
	'.png': 'image/png',
	'.jpg': 'image/jpeg',
	'.webp': 'image/webp',
	'.ico': 'image/x-icon',
	'.woff': 'font/woff',
	'.woff2': 'font/woff2',
	'.map': 'application/json',
	'.txt': 'text/plain;charset=utf-8'
};

/** `vite preview` 404s every asset a later build produced: SvelteKit's preview
 *  plugin registers a `sirv` without `dev`, which snapshots the file list at
 *  startup, and it registers it ahead of Vite's own disk-reading middleware. The
 *  HTML is re-read per request, so a rebuilt SPA serves fresh markup pointing at
 *  hashes the server refuses — a blank page until preview is restarted.
 *
 *  Registering first, from `configurePreviewServer`, puts a disk-backed lookup
 *  ahead of the snapshot. Unknown extensions and non-files fall through, so the
 *  SPA fallback and every other middleware behave as before.
 */
function previewServesCurrentBuild(clientDir: string): Plugin {
	const root = resolvePath(clientDir);
	return {
		name: 'cctui:preview-serves-current-build',
		enforce: 'pre',
		configurePreviewServer(server) {
			server.middlewares.use((req, res, next) => {
				if (req.method !== 'GET' && req.method !== 'HEAD') return next();
				const pathname = new URL(req.url ?? '/', 'http://x').pathname;
				const file = resolvePath(join(root, decodeURIComponent(pathname)));
				if (file !== root && !file.startsWith(root + sep)) return next();
				const type = MIME[extname(file).toLowerCase()];
				if (!type) return next();
				// Under /immutable/ the build on disk is authoritative: falling through
				// lets the stale snapshot stream a file the rebuild deleted, which
				// takes the whole preview process down with an unhandled ENOENT.
				const immutable = pathname.includes('/immutable/');
				let stats;
				try {
					stats = statSync(file);
				} catch {
					if (!immutable) return next();
					res.statusCode = 404;
					return res.end('Not found');
				}
				if (!stats.isFile()) return next();
				res.setHeader('Content-Type', type);
				res.setHeader('Content-Length', stats.size);
				res.setHeader('Cache-Control', immutable ? 'public,max-age=31536000,immutable' : 'no-cache');
				if (req.method === 'HEAD') return res.end();
				createReadStream(file).pipe(res);
			});
		}
	};
}

export default defineConfig({
	plugins: [
		previewServesCurrentBuild(
			fileURLToPath(new URL('./.svelte-kit/output/client', import.meta.url))
		),
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
	},
	// `preview` serves the production build with the same proxy, which is what
	// the journeys replay against: a dev server transforms modules on demand, so
	// a cold first paint can lose a click to hydration.
	preview: {
		host: true,
		port: 5273,
		proxy: devProxy(process.env.CCTUI_PROXY)
	}
});
