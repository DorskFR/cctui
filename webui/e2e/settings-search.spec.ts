import { expect, test, type Page } from '@playwright/test';
import { stubDrawer } from './drawer.fixture';

// Two words that only the "Notify on input needed" row carries, on a page the
// default route does not show: the first keystroke already matches elsewhere
// and makes the page navigate, which is where focus used to be lost.
const QUERY = 'notify inp';
const VIEWPORTS = {
	desktop: { width: 1280, height: 800 },
	phone: { width: 390, height: 844 }
};

test.beforeEach(({ page }) => stubDrawer(page));

async function visibleRowTexts(page: Page): Promise<string[]> {
	return page
		.locator('[data-setting-row]')
		.evaluateAll((rows) =>
			rows
				.filter((r) => (r as HTMLElement).style.display !== 'none')
				.map((r) => (r.textContent ?? '').toLowerCase())
		);
}

for (const [name, size] of Object.entries(VIEWPORTS)) {
	test(`${name}: typing a whole query keeps focus in the settings search and filters live`, async ({
		page
	}) => {
		await page.setViewportSize(size);
		await page.goto('/settings');
		await page.waitForURL(/\/settings\/.+/);
		const search = page.locator('input[type="search"]:visible');
		await expect(search).toHaveCount(1);

		await search.click();
		await expect(search).toBeFocused();
		await page.keyboard.type(QUERY, { delay: 60 });

		await expect(search).toBeFocused();
		await expect(search).toHaveValue(QUERY);
		await expect(page).toHaveURL(/\/settings\/notifications/);
		await expect
			.poll(async () => {
				const texts = await visibleRowTexts(page);
				return texts.length > 0 && texts.every((t) => t.includes('notify') && t.includes('inp'));
			})
			.toBe(true);
	});
}
