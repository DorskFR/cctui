import { expect, test } from '@playwright/test';
import { metrics, session, stubApi, watch } from './tiles.fixture';

const N = Number(process.env.TILES_N ?? 24);
const SESSIONS = Array.from({ length: N }, (_, i) => ({
	...session(i),
	labels: [{ id: `l${i}`, name: 'backend', color: '#8ab4f8' }],
	activity_detail: 'editing src/lib/tiles.ts',
	last_tool_name: 'Edit',
	last_tool_at: new Date().toISOString(),
	todos: [{ content: 'wire the cap', status: 'in_progress' }]
}));

const SHOTS = '/home/dorsk/.claude/artifacts/cctui-wave-023/tiles-crash';

// Deliberately re-derives paneCapacity rather than importing it: the cap must
// hold against the area the browser really laid out, not against the same
// numbers the app reasoned from.
const expectedCap = (w: number, h: number) =>
	Math.min(Math.max(Math.round((w * h) / (950 * 460)), 1), 10);

test.beforeEach(async ({ page }) => {
	await page.setViewportSize({ width: 1920, height: 1080 });
});

test('a persisted tiles choice loads without crashing or scrolling the page', async ({ page }) => {
	test.setTimeout(120_000);
	const errors = watch(page);
	const seen = await stubApi(page, SESSIONS);
	await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'tiles'));

	await page.goto('/sessions', { waitUntil: 'domcontentloaded' });
	await page.waitForSelector('[data-journey="session-tiles"] .tile', { timeout: 20_000 });
	await page.waitForTimeout(3000);

	const m = await metrics(page);
	const historyFetches = seen.size;
	console.log(`1920x1080 ${JSON.stringify(m)} historyFetches=${historyFetches}`);

	expect(errors, 'no uncaught error').toEqual([]);
	expect(m.view).toBe('tiles');
	expect(m.panes).toBeGreaterThan(0);
	expect(m.docScrollH).toBe(m.winH);

	const cap = expectedCap(m.gridW, m.gridH);
	expect(cap, 'a 1080p window is worth four panes').toBe(4);
	expect(m.panes).toBe(Math.min(N, cap));
	expect(historyFetches).toBeLessThanOrEqual(m.panes);
});

test('?view=list rescues a browser stuck in tiles', async ({ page }) => {
	const errors = watch(page);
	await stubApi(page, SESSIONS);
	await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'tiles'));

	await page.goto('/sessions?view=list', { waitUntil: 'domcontentloaded' });
	await page.waitForTimeout(2000);

	const m = await metrics(page);
	expect(errors, 'no uncaught error').toEqual([]);
	expect(m.view).toBe('list');
	expect(m.panes).toBe(0);
});

test('a tiles render that took the tab down last time starts in the list', async ({ page }) => {
	await stubApi(page, SESSIONS);
	await page.addInitScript(() => {
		localStorage.setItem('cctui_list_view', 'tiles');
		sessionStorage.setItem('cctui_tiles_booting', '1');
	});

	await page.goto('/sessions', { waitUntil: 'domcontentloaded' });
	await page.waitForTimeout(2000);

	const m = await metrics(page);
	expect(m.panes).toBe(0);
	expect(await page.evaluate(() => sessionStorage.getItem('cctui_tiles_booting'))).toBeNull();
});

for (const [w, h, tag, want] of [
	[1920, 1080, '1920x1080', 4],
	[2560, 1440, '2560x1440', 8],
	[3440, 1440, '3440x1440-ultrawide', 10],
	[3840, 2160, '3840x2160', 10]
] as [number, number, string, number][]) {
	test(`the cap is ${want} panes at ${tag}`, async ({ page }) => {
		test.setTimeout(120_000);
		const errors = watch(page);
		await page.setViewportSize({ width: w, height: h });
		const seen = await stubApi(page, SESSIONS);
		await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'tiles'));

		await page.goto('/sessions', { waitUntil: 'domcontentloaded' });
		await page.waitForSelector('[data-journey="session-tiles"] .tile', { timeout: 20_000 });
		await page.waitForTimeout(3000);

		const m = await metrics(page);
		console.log(`${tag} ${JSON.stringify(m)} historyFetches=${seen.size}`);
		await page.screenshot({ path: `${SHOTS}/tiles-${tag}.png` });

		expect(errors, 'no uncaught error').toEqual([]);
		expect(m.docScrollH).toBe(m.winH);
		expect(expectedCap(m.gridW, m.gridH), `cap at ${tag}`).toBe(want);
		expect(m.panes).toBe(Math.min(N, want));
		expect(seen.size).toBeLessThanOrEqual(m.panes);

		const chip = page.getByRole('button', { name: `+${N - m.panes} more` });
		await expect(chip).toBeVisible();
		const box = await chip.boundingBox();
		expect(box, 'the overflow chip has a box').not.toBeNull();
		expect(box!.height).toBeGreaterThan(20);
		expect(box!.y).toBeGreaterThanOrEqual(0);
		expect(box!.y + box!.height).toBeLessThanOrEqual(m.winH);

		await chip.click();
		await expect(page.getByRole('button', { name: `session ${N - 1}` })).toBeVisible();
	});
}
