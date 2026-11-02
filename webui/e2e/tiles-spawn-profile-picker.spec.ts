import { expect, test, type Locator, type Page } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';

const MOBILE = { width: 390, height: 844 };
const MACHINE = session(0);

const profiles = Array.from({ length: 12 }, (_, i) => ({
	id: `p${i}`,
	user_id: 'u1',
	name: `Profile ${i}`,
	harness: i % 2 ? 'codex' : 'claude-code',
	account_id: null,
	pool_id: null,
	no_account: false,
	model_alias: null,
	effort: 'medium',
	permission_mode: 'auto',
	service_tier: null,
	context_items: [],
	sort_order: i,
	created_at: '',
	updated_at: ''
}));

async function stub(page: Page) {
	await stubApi(page, [MACHINE]);
	await page.route(
		(url) => url.pathname === '/api/v1/machines/resources',
		(route) =>
			route.fulfill({
				json: [
					{
						machine_id: MACHINE.machine_id,
						name: MACHINE.machine_name,
						display_name: MACHINE.machine_name,
						hue: null,
						liveness: 'online',
						last_seen_at: MACHINE.last_heartbeat,
						resources: null,
						updated_at: null
					}
				]
			})
	);
	await page.route(
		(url) => url.pathname === '/api/v1/sessions/recent-dirs',
		(route) => route.fulfill({ json: [MACHINE.working_dir] })
	);
	const orders: string[][] = [];
	await page.route(
		(url) => url.pathname === '/api/v1/profiles/order',
		(route) => {
			const ids = (route.request().postDataJSON() as { ids: string[] }).ids;
			orders.push(ids);
			return route.fulfill({ json: ids.map((id) => profiles.find((p) => p.id === id)) });
		}
	);
	await page.route(
		(url) => url.pathname === '/api/v1/profiles',
		(route) => route.fulfill({ json: profiles })
	);
	return orders;
}

async function openSpawn(page: Page) {
	await page.setViewportSize(MOBILE);
	await page.goto('/sessions');
	await page.getByRole('button', { name: 'New session' }).first().click();
	const dialog = page.locator('dialog[open]').first();
	await expect(dialog.locator('[data-journey="profiles"]')).toBeVisible();
	return dialog;
}

async function box(locator: Locator) {
	const b = await locator.boundingBox();
	if (!b) throw new Error('element has no bounding box');
	return b;
}

async function inViewport(locator: Locator) {
	await expect(locator).toBeVisible();
	const b = await box(locator);
	expect(b.y).toBeGreaterThanOrEqual(0);
	expect(b.y + b.height).toBeLessThanOrEqual(MOBILE.height);
}

const trigger = (dialog: Locator) => dialog.locator('[data-journey="profiles"] .rs-trigger');
const panel = (page: Page) => page.locator('.rs-panel:popover-open');

test('390px: closed, the picker is one card and Labels/Files/Env vars and Launch need no scroll', async ({ page }) => {
	await stub(page);
	const dialog = await openSpawn(page);

	await expect(trigger(dialog)).toHaveAttribute('aria-expanded', 'false');
	await expect(dialog.locator('[data-journey="profile"]:visible')).toHaveCount(1);
	await expect(trigger(dialog).locator('[data-profile-id="p0"]')).toBeVisible();
	expect((await box(trigger(dialog))).height).toBeLessThan(80);

	await inViewport(dialog.getByText('Labels', { exact: true }).first());
	await inViewport(dialog.getByText('Files', { exact: true }).first());
	await inViewport(dialog.getByText('Env vars', { exact: true }).first());
	await inViewport(dialog.getByRole('button', { name: /^Launch/ }));

	const scrolled = await dialog.evaluate((root) =>
		[root, ...root.querySelectorAll('*')].some((el) => el.scrollTop > 0)
	);
	expect(scrolled).toBe(false);
});

test('390px: the open dropdown scrolls internally and keeps the New profile footer pinned', async ({ page }) => {
	await stub(page);
	const dialog = await openSpawn(page);

	await trigger(dialog).click();
	const pop = panel(page);
	await expect(pop).toBeVisible();
	await expect(trigger(dialog)).toHaveAttribute('aria-expanded', 'true');
	await expect(pop.getByRole('option')).toHaveCount(profiles.length);
	await expect(pop.locator('.rs-trailing').getByRole('button', { name: 'Adjust profile' })).toHaveCount(
		profiles.length
	);

	const list = pop.locator('.rs-list');
	const footer = pop.locator('.rs-footer').getByRole('button', { name: 'New profile' });
	const overflow = await list.evaluate((el) => el.scrollHeight - el.clientHeight);
	expect(overflow).toBeGreaterThan(0);

	const panelBox = await box(pop);
	expect(panelBox.y).toBeGreaterThanOrEqual(0);
	expect(panelBox.y + panelBox.height).toBeLessThanOrEqual(MOBILE.height);

	await inViewport(footer);
	const before = await box(footer);
	await list.evaluate((el) => {
		el.scrollTop = el.scrollHeight;
	});
	await expect.poll(() => list.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
	const after = await box(footer);
	expect(Math.abs(after.y - before.y)).toBeLessThan(1);
	await inViewport(footer);
	await expect(pop.getByRole('option').last()).toBeInViewport();
});

test('390px: picking a card closes the dropdown back to that one card', async ({ page }) => {
	await stub(page);
	const dialog = await openSpawn(page);

	await trigger(dialog).click();
	await panel(page).getByRole('option').filter({ has: page.locator('[data-profile-id="p3"]') }).click();
	await expect(panel(page)).toHaveCount(0);
	await expect(trigger(dialog)).toHaveAttribute('aria-expanded', 'false');
	await expect(trigger(dialog).locator('[data-profile-id="p3"]')).toBeVisible();
	await expect(dialog.locator('[data-journey="profile"]:visible')).toHaveCount(1);
});

test('arrow keys switch profiles on the closed picker without opening it', async ({ page }) => {
	await stub(page);
	const dialog = await openSpawn(page);

	await trigger(dialog).focus();
	await page.keyboard.press('ArrowDown');
	await expect(trigger(dialog).locator('[data-profile-id="p1"]')).toBeVisible();
	await page.keyboard.press('ArrowUp');
	await expect(trigger(dialog).locator('[data-profile-id="p0"]')).toBeVisible();
	await expect(trigger(dialog)).toHaveAttribute('aria-expanded', 'false');
	await expect(panel(page)).toHaveCount(0);
});

test('dragging a row grip in the dropdown persists the new order', async ({ page }) => {
	const orders = await stub(page);
	const dialog = await openSpawn(page);

	await trigger(dialog).click();
	const rows = panel(page).locator('.rs-row');
	const grip = await box(rows.nth(0).locator('.rs-grip'));
	const third = await box(rows.nth(2));
	await page.mouse.move(grip.x + grip.width / 2, grip.y + grip.height / 2);
	await page.mouse.down();
	await page.mouse.move(grip.x + grip.width / 2, third.y + third.height / 2, { steps: 8 });
	await page.mouse.up();

	await expect.poll(() => orders.length).toBe(1);
	expect(orders[0][0]).not.toBe('p0');
	expect(orders[0]).toContain('p0');
	expect(orders[0]).toHaveLength(profiles.length);
});
