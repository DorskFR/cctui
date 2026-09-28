import { expect, test } from '@playwright/test';
import { NARROW, TINY, openDrawer, pageOverflow } from './drawer-header.helpers';

for (const viewport of [NARROW, TINY]) {
	const w = viewport.width;

	test(`${w}px: the drawer header never scrolls the page sideways`, async ({ page }) => {
		await openDrawer(page, viewport);
		expect(await pageOverflow(page)).toBeLessThanOrEqual(1);

		await page.locator('[data-journey="head-details"]').click();
		await expect(page.locator('.metapop')).toBeVisible();
		expect(await pageOverflow(page)).toBeLessThanOrEqual(1);
	});

	test(`${w}px: the logo trigger and its popover stay inside the viewport`, async ({
		page
	}) => {
		await openDrawer(page, viewport);

		const trigger = page.locator('[data-journey="head-details"]');
		await expect(trigger).toBeVisible();
		const head = (await page.locator('[data-journey="header"]').boundingBox())!;
		const t = (await trigger.boundingBox())!;
		expect(t.x).toBeGreaterThanOrEqual(-0.5);
		expect(t.x + t.width).toBeLessThanOrEqual(viewport.width + 0.5);
		expect(t.y).toBeGreaterThanOrEqual(head.y - 0.5);
		expect(t.y + t.height).toBeLessThanOrEqual(head.y + head.height + 0.5);

		await trigger.click();
		const panel = page.locator('.metapop');
		await expect(panel).toBeVisible();
		const p = (await panel.boundingBox())!;
		expect(p.x).toBeGreaterThanOrEqual(-0.5);
		expect(p.x + p.width).toBeLessThanOrEqual(viewport.width + 0.5);
		expect(p.y).toBeGreaterThanOrEqual(-0.5);
		expect(p.y + p.height).toBeLessThanOrEqual(viewport.height + 0.5);
	});

	test(`${w}px: the model chip and Σ live only behind the logo`, async ({ page }) => {
		await openDrawer(page, viewport);

		const row = page.locator('[data-journey="head-meta"] .meta-trail');
		await expect(row.locator('.model')).toBeHidden();
		await expect(row.locator('.tokens')).toBeHidden();
		await expect(row.getByRole('button', { name: /info/i })).toHaveCount(0);

		await page.locator('[data-journey="head-details"]').click();
		await expect(page.locator('.metapop .tokens')).toBeAttached();
	});
}
