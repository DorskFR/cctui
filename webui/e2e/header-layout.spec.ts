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
	await page.waitForTimeout(500);
}

test('desktop: the tail is the status button and the account menu only', async ({ page }, testInfo) => {
	await openSessions(page, DESKTOP);

	const tail = page.locator('header.hd .tail');
	await expect(tail.locator('[data-tsu="ThemePicker"]')).toHaveCount(0);
	await expect(tail.locator('[data-tsu="FontScalePicker"]')).toHaveCount(0);
	expect(await tail.locator(':scope > *').count()).toBeLessThanOrEqual(2);

	const lead = page.locator('header.hd .lead');
	const vers = page.locator('header.hd .vers');
	if ((await vers.count()) === 0) await expect(lead).not.toContainText(/srv v\d/);

	const neighbours = [
		...(await boxes(page.locator('header.hd .tail > *'))),
		...(await boxes(page.locator('header.hd .tabs a')))
	];
	for (const v of await boxes(vers)) {
		expect(neighbours.filter((n) => intersects(v.box, n.box)).map((n) => n.label)).toEqual([]);
	}

	await page.screenshot({
		path: testInfo.outputPath('desktop-1280.png'),
		clip: { x: 0, y: 0, width: DESKTOP.width, height: 80 }
	});
});

test('the account menu carries the version, Appearance and the update check', async ({ page }) => {
	for (const viewport of [DESKTOP, MOBILE]) {
		await openSessions(page, viewport);
		await page.getByRole('button', { name: 'Account menu' }).click();
		const menu = page.getByRole('menu');
		await expect(menu).toContainText(/Version .*Check for updates|Newer version available/);
		await expect(menu.getByRole('menuitem', { name: /Appearance/ })).toBeVisible();
		await page.keyboard.press('Escape');
	}
});

test('Appearance opens the theme and text-size pickers with a link to Settings', async ({ page }) => {
	await openSessions(page, DESKTOP);
	await page.getByRole('button', { name: 'Account menu' }).click();
	await page.getByRole('menuitem', { name: /Appearance/ }).click();
	const dialog = page.getByRole('dialog');
	await expect(dialog.locator('[data-tsu="ThemePicker"]')).toBeVisible();
	await expect(dialog.getByRole('link', { name: 'More in Settings' })).toHaveAttribute(
		'href',
		'/settings/interface#theme'
	);
});

test.describe('touch', () => {
	test.use({ hasTouch: true, isMobile: true, viewport: MOBILE });

	test('every header control answers a 44 px thumb', async ({ page }, testInfo) => {
		await page.goto('/sessions');
		await page.locator('header.hd .pill').waitFor();
		await page.waitForTimeout(500);

		const controls = page.locator('header.hd .tail button, header.hd .tail a');
		const n = await controls.count();
		expect(n).toBeGreaterThan(0);
		for (let i = 0; i < n; i++) {
			const el = controls.nth(i);
			if (!(await el.isVisible())) continue;
			const misses = await el.evaluate((node) => {
				const r = node.getBoundingClientRect();
				const cx = r.left + r.width / 2;
				const cy = r.top + r.height / 2;
				const out: string[] = [];
				for (const [dx, dy] of [[0, -21], [0, 21], [-21, 0], [21, 0]]) {
					const hit = document.elementFromPoint(cx + dx, cy + dy);
					if (!hit || !(node === hit || node.contains(hit))) out.push(`${dx},${dy}`);
				}
				return out;
			});
			expect(misses, `control #${i}`).toEqual([]);
		}

		await page.screenshot({
			path: testInfo.outputPath('mobile-390.png'),
			clip: { x: 0, y: 0, width: MOBILE.width, height: 80 }
		});
	});
});

test('an unknown address shows a way back instead of a bare 404', async ({ page }) => {
	await page.goto('/does-not-exist');
	await expect(page.getByRole('heading', { name: 'Page not found' })).toBeVisible();
	await expect(page.getByRole('link', { name: 'Go to sessions' })).toHaveAttribute('href', '/sessions');
});

test('the system bar carries neither the ? nor a bell button (CCT-1012)', async ({ page }) => {
	await openSessions(page, DESKTOP);
	const tail = page.locator('header.hd .tail');
	await expect(tail.locator('a[href^="/settings/interface#guides"], a[href="/settings/guides"]')).toHaveCount(0);
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
