import { defineConfig } from '@playwright/test';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const webui = resolve(dirname(fileURLToPath(import.meta.url)));

const port = Number(process.env.HEADER_E2E_PORT ?? 5291);
const headerUrl = process.env.HEADER_E2E_URL ?? `http://localhost:${port}`;

export default defineConfig({
	testDir: 'e2e',
	testIgnore: /probe[^\\/]*\.spec\.ts$/,
	fullyParallel: false,
	workers: 3,
	reporter: [['list']],
	// A server already on the default port may be serving another build.
	webServer: process.env.HEADER_E2E_URL
		? undefined
		: {
				command: `npx vite preview --port ${port} --strictPort`,
				url: headerUrl,
				cwd: webui,
				reuseExistingServer: false,
				timeout: 120_000,
				env: { CCTUI_PROXY: process.env.CCTUI_PROXY ?? 'http://localhost:8700' }
			},
	projects: [
		{
			name: 'header',
			workers: 1,
			testMatch: 'header-layout.spec.ts',
			use: { baseURL: headerUrl, storageState: resolve(webui, 'journeys/.auth/state.json') }
		},
		{
			name: 'mobile-fields',
			workers: 1,
			testMatch: 'mobile-inline-fields.spec.ts',
			use: { baseURL: headerUrl, storageState: resolve(webui, 'journeys/.auth/state.json') }
		},
		{
			name: 'drawer-header',
			fullyParallel: true,
			testMatch: [
				'drawer-header-meta.spec.ts',
				'drawer-header-actions.spec.ts',
				'drawer-header-mobile.spec.ts',
				'drawer-typing.spec.ts'
			],
			use: { baseURL: headerUrl }
		},
		{
			name: 'spawn',
			workers: 1,
			testMatch: 'spawn-prompt-history.spec.ts',
			use: { baseURL: process.env.SPAWN_E2E_URL ?? 'http://localhost:5311' }
		},
		{
			name: 'drawer',
			fullyParallel: true,
			testMatch: [
				'drawer-scrollbars.spec.ts',
				'composer-inset.spec.ts',
				'mobile-hscroll.spec.ts',
				'cold-followup.spec.ts',
				'followup-thread.spec.ts',
				'auto-followup.spec.ts'
			],
			use: { baseURL: headerUrl }
		},
		// CI runs this and the two drawer projects: every request is stubbed
		// through page.route, so they need no server, no seeded database and no
		// journeys/.auth/state.json, and any test can run beside any other. Keep
		// them that way.
		{
			name: 'hermetic',
			fullyParallel: true,
			// Anchored to the filename: testMatch sees the absolute path, so a bare
			// `tiles-` also matches every spec under a directory whose name starts with it.
			testMatch: /(^|[\\/])(tiles-|fontscale-|voice-|settings-|redacted-|sessions-|schedules-|webhooks-)[^\\/]*\.spec\.ts$/,
			use: { baseURL: headerUrl }
		},
		{
			name: 'plugins',
			workers: 1,
			testMatch: ['plugin-review-pane.spec.ts', 'plugin-admin.spec.ts'],
			use: { baseURL: process.env.PLUGIN_E2E_URL ?? headerUrl }
		},
		{
			name: 'otel-trace',
			workers: 1,
			testMatch: 'otel-spawn-trace.spec.ts',
			use: { baseURL: process.env.OTEL_E2E_URL ?? headerUrl }
		}
	]
});
