import { expect, test, type Page } from '@playwright/test';
import { session, stubApi, watch } from './tiles.fixture';

const SHOTS = process.env.USAGE_AXIS_SHOTS;

function usage(days: number) {
	const hourly = days === 1;
	const step = hourly ? 3_600_000 : 86_400_000;
	const count = hourly ? 24 : days;
	const now = Date.now();
	return {
		granularity: hourly ? 'hour' : 'day',
		buckets: Array.from({ length: count }, (_, i) => ({
			bucket: new Date(now - (count - 1 - i) * step).toISOString(),
			input: 1_000_000 * (1 + ((i * 7) % 5)),
			output: 400_000 * (1 + ((i * 3) % 4)),
			cache_read: 900_000_000 * (1 + ((i * 5) % 3)),
			cache_creation: 0,
			cost_usd: 1.25 * (i + 1)
		})),
		models: [],
		heatmap: []
	};
}

async function stub(page: Page, settings: Record<string, unknown> = {}) {
	await stubApi(page, [session(0)], { settings });
	await page.route('**/api/v1/sessions/stats/usage**', (route) => {
		const days = Number(new URL(route.request().url()).searchParams.get('days') ?? 30);
		return route.fulfill({ json: usage(days) });
	});
}

const axisBoxes = (page: Page) =>
	page.evaluate(() => {
		const chart = document.querySelector('.chart') as HTMLElement;
		const axis = chart.querySelector('.axis') as HTMLElement;
		const walker = document.createTreeWalker(axis, NodeFilter.SHOW_TEXT);
		const boxes: { text: string; left: number; right: number }[] = [];
		for (let n = walker.nextNode(); n; n = walker.nextNode()) {
			if (!n.textContent?.trim()) continue;
			if (getComputedStyle(n.parentElement!).visibility !== 'visible') continue;
			const r = document.createRange();
			r.selectNodeContents(n);
			const b = r.getBoundingClientRect();
			if (b.width > 0) boxes.push({ text: n.textContent.trim(), left: b.left, right: b.right });
		}
		const c = chart.getBoundingClientRect();
		return { boxes: boxes.sort((a, b) => a.left - b.left), left: c.left, right: c.right };
	});

const peakText = (page: Page) =>
	page.evaluate(() => (document.querySelector('.chart > :last-child') as HTMLElement).innerText);

function assertAxis(m: Awaited<ReturnType<typeof axisBoxes>>, range: string) {
	expect.soft(m.boxes.length, `${range}: labels shown`).toBeGreaterThanOrEqual(2);
	for (let i = 1; i < m.boxes.length; i++) {
		const a = m.boxes[i - 1];
		const b = m.boxes[i];
		expect.soft(a.right, `${range}: "${a.text}" overlaps "${b.text}"`).toBeLessThanOrEqual(b.left);
	}
	expect.soft(m.boxes[0].left, `${range}: first label inside the chart`).toBeGreaterThanOrEqual(m.left - 0.5);
	expect.soft(m.boxes.at(-1)!.right, `${range}: last label inside the chart`).toBeLessThanOrEqual(m.right + 0.5);
}

function assertPeak(text: string, range: string) {
	expect.soft(text, `${range}: peak has no seconds`).not.toMatch(/\d{1,2}:\d{2}:\d{2}/);
	if (range === '24h') expect.soft(text, `${range}: peak names the hour`).toMatch(/\d{1,2}:\d{2}/);
	else expect.soft(text, `${range}: day peak has no time`).not.toMatch(/\d{1,2}:\d{2}/);
}

async function checkRanges(page: Page, tag: string, picker: string) {
	for (const range of ['24h', '7d', '30d']) {
		await page.locator(picker).getByText(range, { exact: true }).click();
		const count = range === '24h' ? 24 : range === '7d' ? 7 : 30;
		await expect(page.locator('.chart .col')).toHaveCount(count);
		await expect.poll(async () => (await axisBoxes(page)).boxes.length).toBeGreaterThanOrEqual(2);
		const m = await axisBoxes(page);
		console.log(`${tag} ${range} width=${Math.round(m.right - m.left)} labels=${m.boxes.map((b) => b.text).join(',')}`);
		if (SHOTS)
			await page.locator('.chart').screenshot({ path: `${SHOTS}/${tag}-${range}.png` });
		assertAxis(m, range);
		assertPeak(await peakText(page), range);
	}
}

test('usage chart axis never overlaps in the 460px stats dock', async ({ page }) => {
	const errors = watch(page);
	await page.setViewportSize({ width: 1600, height: 1000 });
	await stub(page, { statsDock: { enabled: true, side: 'left', width: 460 } });
	await page.goto('/sessions', { waitUntil: 'domcontentloaded' });
	const dock = page.locator('aside.dock');
	await dock.getByRole('button', { name: 'Usage analytics' }).click();
	await expect(dock.locator('.chart')).toBeVisible();
	await checkRanges(page, 'dock-460', 'aside.dock');
	expect(errors).toEqual([]);
});

for (const width of [1600, 460]) {
	test(`usage chart axis never overlaps on the home page at ${width}px`, async ({ page }) => {
		const errors = watch(page);
		await page.setViewportSize({ width, height: 1000 });
		await stub(page);
		await page.goto('/', { waitUntil: 'domcontentloaded' });
		await expect(page.locator('.chart')).toBeVisible({ timeout: 30_000 });
		await checkRanges(page, `home-${width}`, '[data-journey="range"]');
		expect(errors).toEqual([]);
	});
}
