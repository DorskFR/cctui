import { type Page, expect, test } from '@playwright/test';
import { session, stubApi, watch } from './tiles.fixture';

const SESSIONS = [session(1), session(2)];
const [A] = SESSIONS;
const RECONNECT_SCREEN_AFTER_MS = 3 * 60_000;

async function openWithSwitchableServer(page: Page) {
	const server = { down: false };
	await stubApi(page, SESSIONS);
	await page.route('**/api/v1/**', (route) =>
		server.down ? route.fulfill({ status: 503, body: 'Service Unavailable' }) : route.fallback()
	);
	await page.goto('/sessions?view=list');
	const row = page.locator(`[data-session-id="${A.id}"]`);
	await row.waitFor();
	await row.evaluate((el) => {
		el.dataset.mounted = 'original';
	});
	return {
		server,
		row,
		banner: page.getByTestId('reconnect-banner'),
		screen: page.getByTestId('maintenance-screen')
	};
}

test('losing the server keeps the page under a reconnecting banner, and coming back drops it', async ({
	page
}) => {
	test.setTimeout(90_000);
	const errors = watch(page);
	const { server, row, banner, screen } = await openWithSwitchableServer(page);

	// The refused socket keeps reporting failures; /version answering keeps the page up.
	await page.waitForTimeout(4_000);
	await expect(banner).toHaveCount(0);
	await expect(screen).toHaveCount(0);

	server.down = true;
	await expect(banner).toBeVisible({ timeout: 20_000 });
	await expect(banner).toContainText('Reconnecting to the server…');
	await expect(banner).toContainText(/Retrying/);
	await expect(banner.getByRole('button', { name: 'Retry now' })).toBeVisible();
	await expect(screen).toHaveCount(0);
	await expect(row).toHaveAttribute('data-mounted', 'original');

	server.down = false;
	await expect(banner).toHaveCount(0, { timeout: 40_000 });
	await expect(row).toHaveAttribute('data-mounted', 'original');
	expect(new URL(page.url()).pathname).toBe('/sessions');
	expect(errors.filter((e) => !e.includes('503'))).toEqual([]);
});

test('a fresh load with no server shows the full-page screen', async ({ page }) => {
	await stubApi(page, SESSIONS);
	await page.route('**/api/v1/**', (route) => route.fulfill({ status: 503, body: 'Service Unavailable' }));
	await page.goto('/sessions?view=list');
	const screen = page.getByTestId('maintenance-screen');
	await expect(screen).toBeVisible({ timeout: 20_000 });
	await expect(screen).toContainText('cctui is starting up or under maintenance');
	await expect(page.getByTestId('reconnect-banner')).toHaveCount(0);
});

test('staying down past the threshold escalates the banner to the full-page screen', async ({ page }) => {
	test.setTimeout(90_000);
	await page.clock.install();
	const { server, row, banner, screen } = await openWithSwitchableServer(page);

	server.down = true;
	await expect(banner).toBeVisible({ timeout: 20_000 });
	await page.clock.fastForward(RECONNECT_SCREEN_AFTER_MS);
	await expect(screen).toBeVisible({ timeout: 10_000 });
	await expect(banner).toHaveCount(0);

	server.down = false;
	await expect(screen).toHaveCount(0, { timeout: 45_000 });
	await expect(row).toBeVisible();
	expect(new URL(page.url()).pathname).toBe('/sessions');
});
