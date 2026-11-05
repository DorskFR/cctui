import { expect, test, type Page } from '@playwright/test';
import { NARROW, WIDE, openDrawer, rects } from './drawer-header.helpers';

const trailing = (page: Page) =>
	page.locator(
		'[data-journey="header"] .dbar [data-tsu="Toolbar"] > button:not([data-journey="stop"]), [data-journey="header"] .dbar [data-tsu="Toolbar"] > [data-tsu="Popover"]'
	);

for (const viewport of [WIDE, NARROW]) {
	const w = viewport.width;

	test(`${w}px: every trailing header control shares one height and top y`, async ({
		page
	}) => {
		await openDrawer(page, viewport);

		const controls = await rects(trailing(page));
		expect(controls.length).toBeGreaterThanOrEqual(2);
		for (const c of controls) console.log('control', c.label, JSON.stringify(c));

		expect([...new Set(controls.map((c) => Math.round(c.height)))]).toHaveLength(1);
		expect([...new Set(controls.map((c) => Math.round(c.y)))]).toHaveLength(1);
	});

	// A chip action takes its width from `--box-lg` and its height from
	// `--btn-box`; height and y alone do not catch the two disagreeing.
	test(`${w}px: every trailing header control is square and the same width`, async ({
		page
	}) => {
		await openDrawer(page, viewport);

		const controls = await rects(trailing(page));
		expect(controls.length).toBeGreaterThanOrEqual(2);
		for (const c of controls) console.log('control', c.label, JSON.stringify(c));

		expect([...new Set(controls.map((c) => Math.round(c.width)))]).toHaveLength(1);
		for (const c of controls) {
			expect(Math.round(c.width), `${c.label} is square`).toBe(Math.round(c.height));
		}
	});

	test(`${w}px: the header action row never overflows the drawer`, async ({ page }) => {
		await openDrawer(page, viewport);

		const bar = page.locator('[data-journey="header"] .dbar');
		await page.evaluate(() => document.fonts.ready);
		const snapshot = async () =>
			JSON.stringify({
				bar: await bar.boundingBox(),
				overflow: await bar.evaluate((el) => el.scrollWidth - el.clientWidth),
				controls: await rects(trailing(page))
			});
		let previous = '';
		await expect
			.poll(async () => {
				const current = await snapshot();
				const settled = current === previous;
				previous = current;
				return settled;
			})
			.toBe(true);

		const { bar: barBox, overflow, controls } = JSON.parse(previous);
		expect(overflow).toBeLessThanOrEqual(1);
		for (const c of controls) {
			expect(c.x, c.label).toBeGreaterThanOrEqual(barBox.x - 0.5);
			expect(c.x + c.width, c.label).toBeLessThanOrEqual(barBox.x + barBox.width + 0.5);
		}
	});
}

test('the ⋯ menu is one button, so its tooltip cannot land on a sibling', async ({
	page
}) => {
	await openDrawer(page, WIDE);

	const glyph = page.locator('[data-journey="actions"]');
	await expect(glyph).toHaveCount(1);
	await expect(glyph).toHaveAttribute('title', /.+/);
	expect(await glyph.locator('button').count()).toBe(0);

	const trigger = page.locator('[data-journey="header"] .dbar [aria-haspopup="menu"]');
	await expect(trigger).toHaveCount(1);
	expect(await trigger.evaluate((el) => el.tagName)).toBe('BUTTON');
	expect(await trigger.locator('button').count()).toBe(0);
});
