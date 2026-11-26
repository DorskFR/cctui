import { expect, test, type Page } from '@playwright/test';
import { NARROW, WIDE, openDrawer, rects } from './drawer-header.helpers';

const trailing = (page: Page) =>
	page.locator(
		'[data-journey="header"] .dbar [data-tsu="Toolbar"] > button, [data-journey="header"] .dbar [data-tsu="Toolbar"] > [data-tsu="Popover"]'
	);

for (const viewport of [WIDE, NARROW]) {
	const w = viewport.width;
	// Back, Archive and ⋯.
	const minControls = 3;

	test(`${w}px: every trailing header control shares one height and top y`, async ({
		page
	}) => {
		await openDrawer(page, viewport);

		const controls = await rects(trailing(page));
		expect(controls.length).toBeGreaterThanOrEqual(minControls);
		for (const c of controls) console.log('control', c.label, JSON.stringify(c));

		expect([...new Set(controls.map((c) => Math.round(c.height)))]).toHaveLength(1);
		expect([...new Set(controls.map((c) => Math.round(c.y)))]).toHaveLength(1);
	});

	test(`${w}px: every trailing header control is square and the same width`, async ({
		page
	}) => {
		await openDrawer(page, viewport);

		const controls = await rects(trailing(page));
		expect(controls.length).toBeGreaterThanOrEqual(minControls);
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

test('the header keeps Archive inline and renames from the title', async ({ page }) => {
	await openDrawer(page, WIDE);
	const header = page.locator('[data-journey="header"]');
	await expect(header.getByRole('button', { name: 'Archive' })).toBeVisible();
	await expect(header.getByRole('button', { name: 'Rename' })).toHaveCount(0);
	await header.locator('[data-journey="rename"]').click();
	await expect(header.getByRole('textbox', { name: 'Session name' })).toBeVisible();
});

test('nothing covers the drawer: a point on Back hits Back, not the scrim', async ({ page }) => {
	await openDrawer(page, WIDE);
	const back = page.locator('[data-journey="header"]').getByRole('button', { name: 'Back' });
	const box = (await back.boundingBox())!;
	const hit = await page.evaluate(
		([x, y]) => {
			const el = document.elementFromPoint(x, y);
			return { back: el?.closest('button')?.getAttribute('aria-label') ?? null, scrim: !!el?.closest('[data-tsu="Scrim"]') };
		},
		[box.x + box.width / 2, box.y + box.height / 2]
	);
	expect(hit).toEqual({ back: 'Back', scrim: false });
});
