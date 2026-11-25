import { expect, test } from '@playwright/test';
import { openDrawer, rects } from './drawer-header.helpers';

const PHONE = { width: 412, height: 915 };

test('412px: the title gets its own line under at most two header icons', async ({ page }) => {
	await openDrawer(page, PHONE);
	const header = page.locator('[data-journey="header"]');

	const controls = await rects(
		header.locator('.dbar [data-tsu="Toolbar"] > button, .dbar [data-tsu="Toolbar"] > [data-tsu="Popover"]')
	);
	expect(controls.length).toBeLessThanOrEqual(2);

	const bar = await header.locator('.dbar').boundingBox();
	const title = await header.locator('.trow').boundingBox();
	expect(bar && title).toBeTruthy();
	expect(title!.y).toBeGreaterThanOrEqual(bar!.y + bar!.height - 1);
	expect(title!.width).toBeGreaterThan(PHONE.width * 0.8);

	const overflow = await header.locator('.dbar').evaluate((el) => el.scrollWidth - el.clientWidth);
	expect(overflow).toBeLessThanOrEqual(1);
});

test('1280px: the title stays inline in the action row', async ({ page }) => {
	await openDrawer(page, { width: 1280, height: 900 });
	await expect(page.locator('[data-journey="header"] .trow')).toHaveCount(0);
});
