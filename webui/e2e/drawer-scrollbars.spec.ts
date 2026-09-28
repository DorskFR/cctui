import { expect, test, type Page } from '@playwright/test';

const DESKTOP = { width: 1536, height: 900 };
const NARROW = { width: 800, height: 780 };

async function openSessions(page: Page, viewport: { width: number; height: number }) {
	await page.setViewportSize(viewport);
	await page.goto('/sessions');
	await page.locator('[data-journey="search"]').waitFor();
}

async function openDrawer(page: Page, viewport: { width: number; height: number }) {
	await openSessions(page, viewport);
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
}

const htmlOverflowY = (page: Page) =>
	page.evaluate(() => getComputedStyle(document.documentElement).overflowY);

test('desktop: the open drawer leaves only the transcript scrollbar (CCT-1386)', async ({ page }) => {
	await openDrawer(page, DESKTOP);

	expect(await htmlOverflowY(page)).toBe('hidden');

	const overflow = await page
		.locator('.panel-content')
		.evaluate((el) => el.scrollHeight - el.clientHeight);
	expect(overflow).toBeLessThanOrEqual(1);
});

test('desktop: closing the drawer restores the page scrollbar without shifting (CCT-1386)', async ({
	page
}) => {
	await openSessions(page, DESKTOP);
	const app = page.locator('.app');
	const closed = await app.boundingBox();

	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
	const open = await app.boundingBox();

	await page.getByRole('button', { name: 'Back' }).first().click();
	await expect(page.locator('[data-journey="composer"]')).toHaveCount(0);
	const reclosed = await app.boundingBox();

	expect(Math.round(open!.width)).toBe(Math.round(closed!.width));
	expect(Math.round(open!.x)).toBe(Math.round(closed!.x));
	expect(Math.round(reclosed!.width)).toBe(Math.round(closed!.width));
	expect(await htmlOverflowY(page)).not.toBe('hidden');
});

// Needs the tsumikit `.overlay.full-bleed .panel { width: 100vw }` fix: 100vw
// counts the reserved gutter, so the panel's left 8px are clipped.
test.fixme('800px: the full-bleed drawer fits the viewport (CCT-1386)', async ({ page }) => {
	await openDrawer(page, NARROW);
	const panel = await page.locator('.panel').first().boundingBox();
	const inner = await page.evaluate(() => window.innerWidth);
	expect(panel!.x).toBeGreaterThanOrEqual(0);
	expect(panel!.x + panel!.width).toBeLessThanOrEqual(inner);
});
