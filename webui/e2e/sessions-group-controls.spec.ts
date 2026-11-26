import { expect, test } from '@playwright/test';
import { session, stubApi, watch } from './tiles.fixture';

const sessions = [session(1), session(2), session(3)];

test('group header controls stay visible without hover or focus', async ({ page }) => {
	const errors = watch(page);
	await stubApi(page, sessions);
	await page.setViewportSize({ width: 1600, height: 900 });
	await page.goto('/sessions?view=list');
	await page.locator('[data-journey="group-sort"]').first().waitFor();
	await page.mouse.move(1, 1);
	await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());

	const controls = await page.evaluate(() => {
		const opacity = (el: Element | null) => {
			let o = 1;
			for (let n = el; n && n !== document.body; n = n.parentElement)
				o *= parseFloat(getComputedStyle(n).opacity);
			return o;
		};
		return [...document.querySelectorAll('.ssh')].map((h) => ({
			hovered: h.matches(':hover') || h.matches(':focus-within'),
			sort: opacity(h.querySelector('[data-journey="group-sort"]')),
			eye: opacity(h.querySelector('[data-journey="group-hide"]'))
		}));
	});

	expect(controls.length).toBeGreaterThan(0);
	for (const c of controls) expect(c).toEqual({ hovered: false, sort: 1, eye: 1 });
	expect(errors).toEqual([]);
});
