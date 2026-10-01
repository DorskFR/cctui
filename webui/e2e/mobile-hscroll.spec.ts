import { expect, test, type Page } from '@playwright/test';

test.use({ isMobile: true, hasTouch: true, deviceScaleFactor: 3.5 });

const WIDTHS = [360, 412, 800];
const NARROW = { width: 360, height: 800 };

const SCROLLERS = ['html', '.panel-content', '.conv'];

type Probe = { selector: string; overflow: number; scrollLeft: number };

async function probe(page: Page, selectors: string[] = SCROLLERS): Promise<Probe[]> {
	return page.evaluate(
		(list) =>
			list.flatMap((selector) =>
				[...document.querySelectorAll(selector)].map((el) => ({
					selector,
					overflow: el.scrollWidth - el.clientWidth,
					scrollLeft: Math.round(el.scrollLeft)
				}))
			),
		selectors
	);
}

function expectNoHorizontalScroll(probes: Probe[]) {
	expect(probes.length).toBeGreaterThan(0);
	for (const p of probes) {
		expect(p.overflow, `${p.selector} scrollWidth exceeds clientWidth`).toBeLessThanOrEqual(1);
		expect(p.scrollLeft, `${p.selector} is scrolled sideways`).toBe(0);
	}
}

async function openSessions(page: Page, width: number) {
	await page.setViewportSize({ width, height: 800 });
	await page.goto('/sessions');
	await page.locator('[data-journey="search"]').waitFor();
}

async function openDrawer(page: Page, width: number, index = 0) {
	await openSessions(page, width);
	await page.locator('[data-journey="session"] [data-journey="title"]').nth(index).click();
	await page.locator('[data-journey="composer"]').waitFor();
}

for (const width of WIDTHS) {
	test(`${width}px touch: the open drawer never scrolls sideways`, async ({ page }) => {
		await openDrawer(page, width);
		expectNoHorizontalScroll(await probe(page));
	});
}

test('412px touch: every drawer in the list stays free of horizontal scroll', async ({ page }) => {
	await openSessions(page, 412);
	const titles = page.locator('[data-journey="session"] [data-journey="title"]');
	const count = Math.min(await titles.count(), 3);
	expect(count).toBeGreaterThan(0);

	for (let i = 0; i < count; i++) {
		await openDrawer(page, 412, i);
		expectNoHorizontalScroll(await probe(page));
	}
});

test('412px touch: tapping the composer does not pan the drawer sideways', async ({ page }) => {
	await openDrawer(page, 412);
	const field = page.locator('[data-journey="composer"] textarea').first();
	await field.tap();
	await expect(field).toBeFocused();
	expectNoHorizontalScroll(await probe(page));
});

test('360px touch: the sessions list does not scroll sideways', async ({ page }) => {
	await openSessions(page, NARROW.width);
	expectNoHorizontalScroll(await probe(page, ['html']));
});

test('360px touch: the spawn modal does not scroll sideways', async ({ page }) => {
	await openSessions(page, NARROW.width);
	await page.locator('[data-journey="new"]').first().click();
	await page.locator('[data-journey="spawn"]').waitFor();
	expectNoHorizontalScroll(await probe(page, ['html', '[data-journey="spawn"]']));
});

test('360px touch: the settings page does not scroll sideways', async ({ page }) => {
	await page.setViewportSize(NARROW);
	await page.goto('/settings');
	await page.waitForURL(/\/settings\/.+/);
	await page.locator('main.content').waitFor();
	expectNoHorizontalScroll(await probe(page, ['html', '.panel-content']));
});

test('412px touch: icon buttons keep a 44px hit target', async ({ page }) => {
	await openDrawer(page, 412);
	await page.locator('[data-journey="header"]').waitFor();
	const button = page.locator('[data-journey="header"] button[aria-label]').last();
	const box = (await button.boundingBox())!;
	const label = await button.getAttribute('aria-label');

	const hitWidth = await page.evaluate(
		({ box, label }) => {
			const y = box.y + box.height / 2;
			const hits = (x: number) =>
				document.elementFromPoint(x, y)?.closest('button')?.getAttribute('aria-label') === label;
			let left = box.x;
			let right = box.x + box.width;
			while (hits(left - 1) && box.x - left < 32) left -= 1;
			while (hits(right + 1) && right - (box.x + box.width) < 32) right += 1;
			return right - left;
		},
		{ box, label }
	);

	expect(hitWidth).toBeGreaterThanOrEqual(44);
});
