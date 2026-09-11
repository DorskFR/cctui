import { defineConfig } from '@playwright/test';

export default defineConfig({
	testDir: 'e2e',
	fullyParallel: false,
	workers: 1,
	reporter: [['list']],
	use: { baseURL: process.env.SPAWN_E2E_URL ?? 'http://localhost:5311' }
});
