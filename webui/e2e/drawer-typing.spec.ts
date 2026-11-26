import { expect, test } from '@playwright/test';
import { openDrawer } from './drawer-header.helpers';

const KEYBOARD_UP = { width: 504, height: 506 };

test('504x506: typing folds the header and filters away, blurring brings them back', async ({ page }) => {
	await openDrawer(page, KEYBOARD_UP);
	const header = page.locator('[data-journey="header"]');
	const filters = page.locator('[data-journey="filters"]');
	const transcript = page.locator('[data-journey="conversation"] .conv').first();
	const composer = page.locator('[data-journey="message"]');

	await expect(header).toBeVisible();
	await expect(filters).toBeVisible();
	const before = (await transcript.boundingBox())!.height;
	const atBottom = () =>
		transcript.evaluate((el) => Math.abs(el.scrollHeight - el.scrollTop - el.clientHeight) <= 2);

	await composer.focus();
	await expect(header).toBeHidden();
	await expect(filters).toBeHidden();
	await expect.poll(async () => (await transcript.boundingBox())!.height).toBeGreaterThan(before + 40);
	await expect.poll(atBottom).toBe(true);

	await composer.blur();
	await expect(header).toBeVisible();
	await expect(filters).toBeVisible();
	await expect.poll(atBottom).toBe(true);
});

test('1280x900: focusing the composer leaves the chrome in place', async ({ page }) => {
	await openDrawer(page, { width: 1280, height: 900 });
	await page.locator('[data-journey="message"]').focus();
	await expect(page.locator('[data-journey="header"]')).toBeVisible();
	await expect(page.locator('[data-journey="filters"]')).toBeVisible();
});
