import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Node global (no @types/node in this project); only used at build time.
declare const process: { env: Record<string, string | undefined> };

// Client build version, injected at image-build time via the CLIENT_VERSION
// build arg (= the workspace version; see webui/Dockerfile + `make
// ui/image/build`). Falls back to "dev" for local `npm run dev`/`build`.
const clientVersion = process.env.CLIENT_VERSION || 'dev';

export default defineConfig({
	plugins: [sveltekit()],
	define: {
		__CLIENT_VERSION__: JSON.stringify(clientVersion)
	},
	server: {
		host: true,
		port: 5273
	}
});
