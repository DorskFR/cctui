import { expect, test, type Page } from '@playwright/test';
import { NARROW, WIDE, openDrawer, rects } from './drawer-header.helpers';

const trailing = (page: Page) =>
	page.locator(
		'[data-journey="header"] .dbar [data-tsu="Toolbar"] > button, [data-journey="header"] .dbar [data-tsu="Toolbar"] > [data-tsu="Popover"]'
	);

for (const viewport of [WIDE, NARROW]) {
	const w = viewport.width;

	test(`${w}px: every trailing header control shares one height and top y`, async ({
		page
	}) => {
		await openDrawer(page, viewport);

		const controls = await rects(trailing(page));
		expect(controls.length).toBeGreaterThan(2);
		for (const c of controls) console.log('control', c.label, JSON.stringify(c));

		expect([...new Set(controls.map((c) => Math.round(c.height)))]).toHaveLength(1);
		expect([...new Set(controls.map((c) => Math.round(c.y)))]).toHaveLength(1);
	});

	// `chip` and `box` disagreed in the kit: a chip action took its width from
	// `--box-lg` and its height from `--btn-box`, so archive/stop/rename/search
	// rendered 40x36 next to 36x36 popover triggers. Height and y alone did not
	// catch it.
	test(`${w}px: every trailing header control is square and the same width`, async ({
		page
	}) => {
		await openDrawer(page, viewport);

		const controls = await rects(trailing(page));
		expect(controls.length).toBeGreaterThan(2);
		for (const c of controls) console.log('control', c.label, JSON.stringify(c));

		expect([...new Set(controls.map((c) => Math.round(c.width)))]).toHaveLength(1);
		for (const c of controls) {
			expect(Math.round(c.width), `${c.label} is square`).toBe(Math.round(c.height));
		}
	});

	test(`${w}px: the header action row never overflows the drawer`, async ({ page }) => {
		await openDrawer(page, viewport);

		const bar = page.locator('[data-journey="header"] .dbar');
		expect(await bar.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(1);

		const barBox = (await bar.boundingBox())!;
		for (const c of await rects(trailing(page))) {
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
