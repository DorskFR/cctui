import { expect, test } from '@playwright/test';
import { openDrawer } from './drawer-header.helpers';

const PHONE = { width: 412, height: 915 };

test('412px: the title shares the one header row and truncates', async ({ page }) => {
	await openDrawer(page, PHONE);
	const header = page.locator('[data-journey="header"]');

	const bar = await header.locator('.dbar').boundingBox();
	const title = await header.locator('.dtitle').boundingBox();
	expect(bar && title).toBeTruthy();
	expect(title!.y).toBeGreaterThanOrEqual(bar!.y - 1);
	expect(title!.y + title!.height).toBeLessThanOrEqual(bar!.y + bar!.height + 1);
	await expect(header.locator('.trow')).toHaveCount(0);

	const overflow = await header.locator('.dbar').evaluate((el) => el.scrollWidth - el.clientWidth);
	expect(overflow).toBeLessThanOrEqual(1);
});
