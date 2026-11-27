import { expect, test } from '@playwright/test';
import { ALL_FEATURES } from '../src/lib/features.fixture';
import { session, stubApi } from './tiles.fixture';

const langfuse = { available: false, host: null, public_host: null, project_id: null };

test('a stale sign-in is told to sign in again before adding a passkey', async ({ page }) => {
	const calls: string[] = [];
	await stubApi(page, [session(1)]);
	await page.route('**/api/v1/**', async (route) => {
		const req = route.request();
		const path = new URL(req.url()).pathname.replace('/api/v1', '');
		calls.push(`${req.method()} ${path}`);
		if (path === '/capabilities') {
			return route.fulfill({
				json: { langfuse, distribution: 'full', workspace: true, experimental: false, features: ALL_FEATURES, switchable: ALL_FEATURES }
			});
		}
		if (path === '/auth/passkey/config') return route.fulfill({ json: { available: true, enrolled: true, auto_prompt: false } });
		if (path === '/passkeys') return route.fulfill({ json: { passkeys: [] } });
		if (path === '/passkeys/register/start') {
			return route.fulfill({
				status: 403,
				json: { error: 'sign in again to add a passkey: it needs a sign-in from the last 10 minutes' }
			});
		}
		if (path === '/auth/logout') return route.fulfill({ status: 204 });
		return route.fallback();
	});

	await page.goto('/settings/accounts#security');
	await page.getByRole('button', { name: 'Enrol', exact: true }).click();

	const notice = page.getByTestId('passkey-sign-in-again');
	await expect(notice).toBeVisible();
	await expect(notice).toContainText('needs a sign-in from the last 10 minutes');
	expect(calls).not.toContain('POST /auth/logout');
	expect(calls).not.toContain('POST /passkeys/register/finish');

	await notice.getByRole('button', { name: 'Sign in again' }).click();
	await expect.poll(() => calls).toContain('POST /auth/logout');
});
