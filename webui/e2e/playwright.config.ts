import { defineConfig } from '@playwright/test';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const webui = resolve(dirname(fileURLToPath(import.meta.url)), '..');

const port = Number(process.env.HEADER_E2E_PORT ?? 5291);
const url = process.env.HEADER_E2E_URL ?? `http://localhost:${port}`;

export default defineConfig({
	testDir: '.',
	// Concurrent worktrees: a shared default port serves another branch's build.
	webServer: process.env.HEADER_E2E_URL
		? undefined
		: {
				command: `npx vite preview --port ${port} --strictPort`,
				url,
				cwd: webui,
				reuseExistingServer: false,
				timeout: 120_000,
				env: { CCTUI_PROXY: process.env.CCTUI_PROXY ?? 'http://localhost:8700' }
			},
	use: { baseURL: url, storageState: resolve(webui, 'journeys/.auth/state.json') },
	reporter: [['list']]
});
