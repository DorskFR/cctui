import { expect, test, type Page } from '@playwright/test';

const DESKTOP = { width: 1536, height: 900 };

async function openDrawer(page: Page) {
	await page.setViewportSize(DESKTOP);
	await page.goto('/sessions');
	await page.locator('[data-journey="search"]').waitFor();
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
}

test('desktop: the composer field keeps an inset from the drawer edges (CCT-1385)', async ({
	page
}) => {
	await openDrawer(page);
	const composer = page.locator('[data-journey="composer"]');
	const outer = await composer.boundingBox();
	const field = await composer.locator('[data-tsu="InputGroup"], textarea').first().boundingBox();

	expect(field!.x - outer!.x).toBeGreaterThanOrEqual(8);
	expect(outer!.x + outer!.width - (field!.x + field!.width)).toBeGreaterThanOrEqual(8);
	expect(outer!.y + outer!.height - (field!.y + field!.height)).toBeGreaterThanOrEqual(8);
});
