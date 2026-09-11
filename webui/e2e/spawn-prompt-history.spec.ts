import { expect, test, type Locator, type Page } from '@playwright/test';

const APP = process.env.SPAWN_E2E_URL ?? 'http://localhost:5311';
const TOKEN = process.env.SPAWN_E2E_TOKEN ?? 'dev-admin';
const SHOTS = process.env.SPAWN_E2E_SHOTS ?? '/home/dorsk/.claude/artifacts/hotfix-spawn';

const WIDTHS = [
	{ name: 'desktop', width: 1280, height: 800 },
	{ name: 'mobile', width: 390, height: 844 }
];

async function login(page: Page) {
	const res = await page.request.post(`${APP}/api/v1/auth/login`, { data: { token: TOKEN } });
	expect(res.ok(), `login failed with ${res.status()}`).toBe(true);
}

async function openSpawn(page: Page) {
	await page.goto(`${APP}/sessions`);
	await page.getByRole('button', { name: 'New session' }).first().click();
	// The <dialog> fills the viewport; the card inside it is the modal's real box.
	const card = page.locator('dialog[open] .sheet').first();
	await expect(card).toBeVisible();
	return card;
}

async function seedHistory(page: Page, prompts: string[]) {
	await page.evaluate((list) => {
		localStorage.setItem('cctui_prompt_history', JSON.stringify(list));
	}, prompts);
}

function box(locator: Locator) {
	return locator.boundingBox().then((b) => {
		if (!b) throw new Error('element has no bounding box');
		return b;
	});
}

async function openHistory(page: Page) {
	await page.getByRole('button', { name: 'Recent prompts' }).first().click();
	const menu = page.locator('[role="menu"][popover]:popover-open[aria-label="Recent prompts"]');
	await expect(menu).toBeVisible();
	return menu;
}

for (const vp of WIDTHS) {
	test.describe(`${vp.name} ${vp.width}x${vp.height}`, () => {
		test.use({ viewport: { width: vp.width, height: vp.height } });

		test('the open prompt-history panel stays inside the modal', async ({ page }) => {
			await login(page);
			await page.goto(`${APP}/sessions`);
			await seedHistory(page, ['first recalled prompt', 'second recalled prompt\nwith a second line']);
			const modal = await openSpawn(page);
			const menu = await openHistory(page);

			const m = await box(modal);
			const p = await box(menu);
			expect.soft(p.x, 'panel left edge').toBeGreaterThanOrEqual(m.x);
			expect.soft(p.y, 'panel top edge').toBeGreaterThanOrEqual(m.y);
			expect.soft(p.x + p.width, 'panel right edge').toBeLessThanOrEqual(m.x + m.width);
			expect.soft(p.y + p.height, 'panel bottom edge').toBeLessThanOrEqual(m.y + m.height);

			await page.screenshot({ path: `${SHOTS}/after-${vp.name}-${vp.width}.png` });
		});

		test('the modal fits the viewport and opens no nested dialog', async ({ page }) => {
			await login(page);
			await page.goto(`${APP}/sessions`);
			await seedHistory(page, ['first recalled prompt']);
			const modal = await openSpawn(page);
			await openHistory(page);

			const m = await box(modal);
			expect.soft(m.x).toBeGreaterThanOrEqual(0);
			expect.soft(m.y).toBeGreaterThanOrEqual(0);
			expect.soft(m.x + m.width).toBeLessThanOrEqual(vp.width);
			expect.soft(m.y + m.height).toBeLessThanOrEqual(vp.height);

			const dialogsOpen = await page.locator('dialog[open]').count();
			expect(dialogsOpen).toBeLessThanOrEqual(1);
		});

		test('prompt history is reachable and applies the picked entry', async ({ page }) => {
			await login(page);
			await page.goto(`${APP}/sessions`);
			await seedHistory(page, ['older prompt', 'newest prompt']);
			await openSpawn(page);
			await openHistory(page);

			await page.getByRole('menuitem', { name: 'older prompt' }).click();
			const prompt = page.locator('#sp-prompt');
			await expect(prompt).toHaveValue('older prompt');
		});
	});
}
