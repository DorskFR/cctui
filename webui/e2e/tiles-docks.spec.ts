import { expect, test } from '@playwright/test';
import { metrics, session, stubApi, transcript, watch } from './tiles.fixture';

const N = Number(process.env.TILES_N ?? 24);
const SESSIONS = Array.from({ length: N }, (_, i) => ({
	...session(i),
	labels: [{ id: `l${i}`, name: 'backend', color: '#8ab4f8' }],
	activity_detail: 'editing src/lib/tiles.ts',
	last_tool_name: 'Edit',
	last_tool_at: new Date().toISOString(),
	todos: [{ content: 'wire the cap', status: 'in_progress' }]
}));

const SHOTS = '/home/dorsk/.claude/artifacts/cctui-tiles-chrome';

const DOCKED = {
	spawnDock: { enabled: true, side: 'right' },
	statsDock: { enabled: true, side: 'left' }
};

const stub = (nav: 'top' | 'bottom') => ({
	settings: { ...DOCKED, display: { nav } },
	conversation: transcript
});

const VIEWPORTS = [
	{ w: 1920, h: 1080, tag: '1920x1080', panes: 4, grid: [2, 2] },
	// A real FHD browser window: 1080 less tabs and the address bar.
	{ w: 1920, h: 960, tag: '1920x960', panes: 4, grid: [2, 2] },
	{ w: 3440, h: 1440, tag: '3440x1440', panes: 10, grid: [5, 2] }
] as const;

for (const nav of ['bottom', 'top'] as const) {
	for (const { w, h, tag, panes, grid } of VIEWPORTS) {
		test(`tiles hide both docks and never scroll the page — ${nav} nav, ${tag}`, async ({
			page
		}) => {
			test.setTimeout(180_000);
			const errors = watch(page);
			await page.setViewportSize({ width: w, height: h });
			await stubApi(page, SESSIONS, stub(nav));
			await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'tiles'));

			await page.goto('/sessions', { waitUntil: 'domcontentloaded' });
			await page.waitForSelector('[data-journey="session-tiles"] .tile', { timeout: 30_000 });
			await page.waitForTimeout(4000);

			const m = await metrics(page);
			console.log(`tiles ${nav}-nav ${tag} ${JSON.stringify(m)}`);
			await page.screenshot({ path: `${SHOTS}/tiles-docks-${nav}-${tag}.png` });

			expect(errors, 'no uncaught error').toEqual([]);
			expect(m.view).toBe('tiles');
			expect(m.panes).toBeGreaterThan(0);

			expect(m.docks, 'no docked panel in tiles').toBe(0);
			await expect(page.locator('aside.dock')).toHaveCount(0);

			expect(m.scrollH, 'document does not scroll vertically').toBeLessThanOrEqual(m.winH);
			expect(m.scrollW, 'document does not scroll horizontally').toBeLessThanOrEqual(m.winW);

			expect(m.panes, `${tag} mounts ${panes} panes`).toBe(panes);
			expect([m.cols, m.rows], `${tag} grid shape`).toEqual([grid[0], grid[1]]);

			// The overflow chip lives in the toolbar, so the grid starts against it.
			expect(m.gridTop - m.barBottom, 'no empty band above the grid').toBeLessThanOrEqual(6);
			expect(m.gridTop).toBeGreaterThanOrEqual(m.barBottom - 1);

			// `overflow: clip` hides a too-tall last row from scrollHeight, so
			// measure the bottom row's composer against the chrome directly.
			const floor = nav === 'bottom' ? m.navTop : m.winH;
			expect(m.lastComposerBottom, 'a composer was measured').toBeGreaterThan(0);
			expect(m.lastComposerBottom, 'the bottom row is not clipped').toBeLessThanOrEqual(
				floor + 1
			);

			const scrolled = await page.evaluate(() => {
				const tile = document.querySelector('[data-journey="session-tiles"] .tile');
				const el = [...(tile?.querySelectorAll('*') ?? [])].find(
					(n) => n.scrollHeight > n.clientHeight + 8
				);
				return !!el;
			});
			expect(scrolled, 'a tile scrolls its own transcript').toBe(true);

			const newBtn = page.locator('[data-journey="new"]');
			await expect(newBtn, 'the docked form is replaced by "+ New"').toBeVisible();
			await newBtn.click();
			await expect(page.locator('[data-journey="spawn"]')).toBeVisible();
		});

		test(`list mode keeps both docks — ${nav} nav, ${tag}`, async ({ page }) => {
			test.setTimeout(180_000);
			await page.setViewportSize({ width: w, height: h });
			await stubApi(page, SESSIONS, stub(nav));
			await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'list'));

			await page.goto('/sessions', { waitUntil: 'domcontentloaded' });
			await expect(page.locator('aside.dock')).toHaveCount(2, { timeout: 30_000 });

			const m = await metrics(page);
			expect(m.panes, 'no tiles in list mode').toBe(0);
			expect(
				await page.evaluate(
					() => getComputedStyle(document.querySelector('main.content')!).paddingLeft
				)
			).not.toBe('0px');
		});
	}
}

test('leaving tiles restores the docks the settings still ask for', async ({ page }) => {
	test.setTimeout(180_000);
	await page.setViewportSize({ width: 1920, height: 1080 });
	await stubApi(page, SESSIONS, stub('bottom'));
	await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'tiles'));

	await page.goto('/sessions', { waitUntil: 'domcontentloaded' });
	await page.waitForSelector('[data-journey="session-tiles"] .tile', { timeout: 30_000 });
	await expect(page.locator('aside.dock')).toHaveCount(0);

	await page.goto('/sessions?view=list', { waitUntil: 'domcontentloaded' });
	await expect(page.locator('aside.dock')).toHaveCount(2, { timeout: 30_000 });
});
