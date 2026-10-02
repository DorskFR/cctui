import { expect, test, type Page } from '@playwright/test';
import { stubDrawer, verticalOverflow } from './drawer.fixture';

const DESKTOP = { width: 1536, height: 900 };
const NARROW = { width: 800, height: 780 };
const MOBILE = { width: 390, height: 844 };

async function openSessions(
	page: Page,
	viewport: { width: number; height: number },
	opts: { ask?: boolean } = {}
) {
	await stubDrawer(page, opts);
	await page.setViewportSize(viewport);
	await page.goto('/sessions');
	await page.locator('[data-journey="search"]').waitFor();
}

async function openDrawer(
	page: Page,
	viewport: { width: number; height: number },
	opts: { ask?: boolean } = {}
) {
	await openSessions(page, viewport, opts);
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
}

async function expectPanelFits(page: Page) {
	const panels = await verticalOverflow(page, '.panel-content');
	expect(panels).toHaveLength(1);
	expect(panels[0], 'the drawer panel never scrolls itself').toBeLessThanOrEqual(1);
}

const htmlOverflowY = (page: Page) =>
	page.evaluate(() => getComputedStyle(document.documentElement).overflowY);

test('desktop: the open drawer leaves only the transcript scrollbar', async ({ page }) => {
	await openDrawer(page, DESKTOP);

	expect(await htmlOverflowY(page)).toBe('hidden');

	const overflow = await page
		.locator('.panel-content')
		.evaluate((el) => el.scrollHeight - el.clientHeight);
	expect(overflow).toBeLessThanOrEqual(1);
});

test('desktop: closing the drawer restores the page scrollbar without shifting', async ({ page }) => {
	await openSessions(page, DESKTOP);
	const app = page.locator('.app');
	const closed = await app.boundingBox();

	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
	const open = await app.boundingBox();

	await page.getByRole('button', { name: 'Back' }).first().click();
	await expect(page.locator('[data-journey="composer"]')).toHaveCount(0);
	const reclosed = await app.boundingBox();

	expect(Math.round(open!.width)).toBe(Math.round(closed!.width));
	expect(Math.round(open!.x)).toBe(Math.round(closed!.x));
	expect(Math.round(reclosed!.width)).toBe(Math.round(closed!.width));
	expect(await htmlOverflowY(page)).not.toBe('hidden');
});

test('800px: the full-bleed drawer fits the viewport', async ({ page }) => {
	await openDrawer(page, NARROW);
	const panel = await page.locator('.panel').first().boundingBox();
	const inner = await page.evaluate(() => window.innerWidth);
	expect(panel!.x).toBeGreaterThanOrEqual(0);
	expect(panel!.x + panel!.width).toBeLessThanOrEqual(inner);
});

for (const [name, viewport] of [
	['desktop', DESKTOP],
	['800px', NARROW],
	['390px', MOBILE]
] as const) {
	test(`${name}: only the transcript scrolls, the panel is exactly the viewport`, async ({ page }) => {
		await openDrawer(page, viewport);
		await expect(page.locator('.conv .line').first()).toBeVisible();

		await expectPanelFits(page);
		const [conv] = await verticalOverflow(page, '.conv');
		expect(conv, 'the long transcript scrolls inside .conv').toBeGreaterThan(0);

		const composer = (await page.locator('[data-journey="composer"]').boundingBox())!;
		expect(composer.y + composer.height).toBeLessThanOrEqual(viewport.height + 0.5);
		const header = (await page.locator('[data-journey="header"]').boundingBox())!;
		expect(header.y).toBeGreaterThanOrEqual(-0.5);
	});
}

test('tiles: no tile scrolls itself around its transcript', async ({ page }) => {
	await stubDrawer(page);
	await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'tiles'));
	await page.setViewportSize({ width: 1920, height: 1080 });
	await page.goto('/sessions');
	await page.locator('[data-journey="session-tiles"] .tile .conv .line').first().waitFor();

	const tiles = await verticalOverflow(page, '[data-journey="session-tiles"] .tile');
	expect(tiles.length).toBeGreaterThan(0);
	for (const t of tiles) expect(t).toBeLessThanOrEqual(1);
});

test('desktop: picking an ask option keeps the drawer on screen', async ({ page }) => {
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(e.message));
	await openDrawer(page, DESKTOP, { ask: true });

	const options = page.locator('.ask [role="radiogroup"] label');
	await expect(options).toHaveCount(5);
	for (const i of [0, 3]) {
		await options.nth(i).click();
		await expect(options.nth(i).locator('input')).toBeChecked();

		expect(await page.locator('.panel-content').evaluate((el) => el.scrollTop)).toBe(0);
		await expectPanelFits(page);
		await expect(page.locator('[data-journey="header"]')).toBeInViewport();
		await expect(page.locator('[data-journey="composer"]')).toBeInViewport();
		await expect(page.locator('.ask')).toBeInViewport();
	}
	expect(errors).toEqual([]);
});
