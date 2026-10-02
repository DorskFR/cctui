import { test as base } from '@playwright/test';
import { localToken } from '../scripts/local-token.mjs';

let current = '';

/** The admin token of the live local stack, for tests that cannot be stubbed. */
export function adminToken(): string {
	return current;
}

// Resolved per test, never at module load: a throw during collection aborts
// every project, including the hermetic ones that need no token at all.
export const test = base.extend<{ liveStack: void }>({
	liveStack: [
		async ({}, use, testInfo) => {
			try {
				current = process.env.PLUGIN_E2E_TOKEN ?? process.env.SPAWN_E2E_TOKEN ?? localToken();
			} catch {
				current = '';
			}
			testInfo.skip(!current, 'no admin token: set CCTUI_TOKEN or run `make local/up`');
			await use();
		},
		{ auto: true }
	]
});

export { expect } from '@playwright/test';
