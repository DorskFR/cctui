import { expect, test, type Page } from '@playwright/test';

const MOBILE = { width: 390, height: 844 };

async function openSessions(page: Page) {
	await page.setViewportSize(MOBILE);
	await page.goto('/sessions');
	await page.locator('[data-journey="search"]').waitFor();
}

async function openDrawer(page: Page) {
	await openSessions(page);
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
}

test('390px: the search box keeps its tools on the same row', async ({ page }) => {
	await openSessions(page);
	const search = await page.locator('[data-journey="search"]').boundingBox();
	const more = await page.locator('[data-journey="options"]').boundingBox();
	expect(search && more).toBeTruthy();
	expect(Math.abs(search!.y + search!.height / 2 - (more!.y + more!.height / 2))).toBeLessThan(8);
});

test('390px: the composer is one full-width field with attach and send inside it', async ({ page }) => {
	await openDrawer(page);
	const composer = page.locator('[data-journey="composer"]');
	const field = await composer.locator('textarea').boundingBox();
	const send = await composer.getByRole('button', { name: /send/i }).first().boundingBox();
	const inner = await page.evaluate(() => window.innerWidth);
	expect(field!.width).toBeGreaterThan(0.85 * inner);
	expect(send!.y).toBeGreaterThanOrEqual(field!.y - 1);
	expect(send!.y + send!.height).toBeLessThanOrEqual(field!.y + field!.height + 1);
});

test('390px: the drawer never scrolls sideways', async ({ page }) => {
	await openDrawer(page);
	const overflow = await page.evaluate(() => {
		const el = document.querySelector('.panel-content');
		return el ? el.scrollWidth - el.clientWidth : 0;
	});
	expect(overflow).toBeLessThanOrEqual(1);
});
