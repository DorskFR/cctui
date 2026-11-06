import { expect, test } from '@playwright/test';
import { session, stubApi, watch } from './tiles.fixture';

const SESSIONS = [session(1), session(2)];
const [A] = SESSIONS;

test('the backend going away shows the maintenance screen, and coming back restores the page', async ({
	page
}) => {
	const errors = watch(page);
	let down = false;
	await stubApi(page, SESSIONS);
	await page.route('**/api/v1/**', (route) =>
		down ? route.fulfill({ status: 503, body: 'Service Unavailable' }) : route.fallback()
	);

	await page.goto('/sessions?view=list');
	const row = page.locator(`[data-session-id="${A.id}"]`);
	await row.waitFor();
	const screen = page.getByTestId('maintenance-screen');

	// The refused socket keeps reporting failures; /version answering keeps the page up.
	await page.waitForTimeout(4_000);
	await expect(screen).toHaveCount(0);
	await expect(row).toBeVisible();

	down = true;
	await expect(screen).toBeVisible({ timeout: 20_000 });
	await expect(screen).toContainText('cctui is starting up or under maintenance');
	await expect(screen).toContainText(/Retrying/);
	expect(new URL(page.url()).pathname).toBe('/sessions');

	down = false;
	await expect(screen).toHaveCount(0, { timeout: 20_000 });
	await expect(row).toBeVisible();
	expect(new URL(page.url()).pathname).toBe('/sessions');
	expect(errors.filter((e) => !e.includes('503'))).toEqual([]);
});
