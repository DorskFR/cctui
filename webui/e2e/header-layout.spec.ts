import { expect, test, type Locator, type Page } from '@playwright/test';

type Box = { x: number; y: number; width: number; height: number };

const DESKTOP = { width: 1280, height: 800 };
const MOBILE = { width: 390, height: 844 };

const intersects = (a: Box, b: Box) =>
	a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

async function boxes(scope: Locator): Promise<{ label: string; box: Box }[]> {
	const out: { label: string; box: Box }[] = [];
	const n = await scope.count();
	for (let i = 0; i < n; i++) {
		const el = scope.nth(i);
		if (!(await el.isVisible())) continue;
		const box = await el.boundingBox();
		if (!box || box.width === 0 || box.height === 0) continue;
		out.push({ label: (await el.evaluate((e) => e.className.toString())) || `#${i}`, box });
	}
	return out;
}

async function openSessions(page: Page, viewport: { width: number; height: number }) {
	await page.setViewportSize(viewport);
	await page.goto('/sessions');
	await page.locator('header.hd .pill').waitFor();
	await page.evaluate(() => document.fonts.ready);
	let prev = '';
	await expect
		.poll(async () => {
			const key = await page
				.locator('header.hd')
				.evaluate((hd) =>
					JSON.stringify([...hd.querySelectorAll('*')].map((e) => e.getBoundingClientRect().toJSON()))
				);
			const same = key === prev;
			prev = key;
			return same;
		}, { message: 'the header settles' })
		.toBe(true);
}

test('desktop: the bar carries no version block', async ({ page }, testInfo) => {
	await openSessions(page, DESKTOP);
	await expect(page.locator('header.hd .vers')).toHaveCount(0);
	await expect(page.locator('header.hd')).not.toContainText(/srv v\d/);

	const tail = await boxes(page.locator('header.hd .tail > *'));
	const tabs = await boxes(page.locator('header.hd .tabs a'));
	for (const t of tail) for (const n of tabs) expect(intersects(t.box, n.box)).toBe(false);

	await page.screenshot({
		path: testInfo.outputPath('desktop-1280.png'),
		clip: { x: 0, y: 0, width: DESKTOP.width, height: 80 }
	});
});

test('mobile: the account menu carries the version', async ({ page }, testInfo) => {
	await openSessions(page, MOBILE);
	await expect(page.locator('header.hd .vers')).toHaveCount(0);
	await page.locator('header.hd .pill').click();
	await expect(page.getByRole('menuitem', { name: /v(dev|\d+\.\d+\.\d+)/ })).toBeVisible();

	await page.screenshot({
		path: testInfo.outputPath('mobile-390.png'),
		clip: { x: 0, y: 0, width: MOBILE.width, height: 80 }
	});
});

test('mobile: header controls give a 44px target and fold the status chips', async ({ page }) => {
	await openSessions(page, MOBILE);
	await expect(page.locator('header.hd .gauge')).toBeHidden();
	for (const sel of ['header.hd .pill', 'header.hd .status']) {
		const el = page.locator(sel);
		if ((await el.count()) === 0) continue;
		const box = (await el.boundingBox()) as Box;
		expect(box.height, sel).toBeGreaterThanOrEqual(44);
		expect(box.width, sel).toBeGreaterThanOrEqual(44);
	}
});

test('mobile: the versions hidden from the bar stay readable in Settings › Interface', async ({
	page
}) => {
	await page.setViewportSize(MOBILE);
	await page.goto('/settings/interface');
	const body = page.locator('body');
	await expect(body).toContainText(/srv \d+\.\d+\.\d+/);
	await expect(body).toContainText(/\bui (dev|\d+\.\d+\.\d+)/);
});

test('the system bar carries neither the ? nor a bell button (CCT-1012)', async ({ page }) => {
	await openSessions(page, DESKTOP);
	const tail = page.locator('header.hd .tail');
	await expect(tail.locator('a[href="/settings/guides"]')).toHaveCount(0);
	await expect(tail.getByText('?', { exact: true })).toHaveCount(0);
	await expect(tail.getByText('🔔')).toHaveCount(0);
	await expect(tail.getByText('🔕')).toHaveCount(0);
});

test('the user pill carries no notification bell at either width', async ({ page }) => {
	for (const viewport of [DESKTOP, MOBILE]) {
		await openSessions(page, viewport);
		await expect(page.locator('header.hd .pill .bell')).toHaveCount(0);
	}
});
