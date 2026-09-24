import { expect, test, type Locator, type Page } from '@playwright/test';

const MOBILE = { width: 390, height: 844 };
const FIELDS = 'input:not([type="hidden"]):not([type="checkbox"]):not([type="radio"]):not([type="file"]), textarea';

async function narrowFields(page: Page, scope: Locator): Promise<string[]> {
	await expect(scope.locator(FIELDS).first()).toBeVisible();
	const inner = await page.evaluate(() => window.innerWidth);
	const out: string[] = [];
	for (const el of await scope.locator(FIELDS).all()) {
		if (!(await el.isVisible())) continue;
		const { width, name } = await el.evaluate((e) => ({
			width: e.clientWidth,
			name: e.id || e.getAttribute('aria-label') || e.getAttribute('placeholder') || e.tagName
		}));
		if (width < 0.9 * inner) out.push(`${name}: ${width}px < 0.9 × ${inner}px`);
	}
	return out;
}

async function openSessions(page: Page) {
	await page.setViewportSize(MOBILE);
	await page.goto('/sessions');
	await page.locator('[data-journey="search"]').waitFor();
}

test('390px: the sessions search box is full width', async ({ page }) => {
	await openSessions(page);
	expect(await narrowFields(page, page.locator('[data-journey="search"]'))).toEqual([]);
});

test('390px: the composer textarea is full width', async ({ page }) => {
	await openSessions(page);
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	const composer = page.locator('[data-journey="composer"]');
	await composer.waitFor();
	expect(await narrowFields(page, composer)).toEqual([]);
});

test('390px: the spawn modal fields are full width', async ({ page }) => {
	await openSessions(page);
	await page.locator('[data-journey="new"]').click();
	const where = page.locator('[data-journey="where"]');
	await where.waitFor();
	expect(await narrowFields(page, page.locator('[data-journey="spawn"]'))).toEqual([]);
});
