import { expect, test, type Locator, type Page } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';

const MOBILE = { width: 390, height: 844 };
const MACHINE = session(0);

const makeProfiles = (count: number) =>
	Array.from({ length: count }, (_, i) => ({
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
		sort_order: count - i,
		created_at: '',
		updated_at: ''
	})).reverse();
const profiles = makeProfiles(12);

async function stub(page: Page, list = profiles) {
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
	const writes: string[] = [];
	await page.route(
		(url) => url.pathname.startsWith('/api/v1/profiles'),
		(route) => {
			const req = route.request();
			if (req.method() !== 'GET') {
				writes.push(`${req.method()} ${new URL(req.url()).pathname}`);
				return route.fulfill({ json: { ...list[0], id: 'p-new', name: 'Reviewer' } });
			}
			return route.fulfill({ json: list });
		}
	);
	return writes;
}

async function openSpawn(page: Page) {
	await page.setViewportSize(MOBILE);
	await page.goto('/sessions');
	await page.getByRole('button', { name: 'New session' }).first().click();
	const dialog = page.locator('dialog[open]').first();
	await expect(dialog.locator('[data-journey="profiles"]')).toBeVisible();
	// Until the sheet's slide-in ends, the <dialog> is scrolled by the opening focus.
	await dialog.evaluate((el) =>
		Promise.all(
			el
				.getAnimations({ subtree: true })
				.filter((a) => a.effect?.getComputedTiming().endTime !== Infinity)
				.map((a) => a.finished.catch(() => undefined))
		)
	);
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
		[root, ...root.querySelectorAll('*')]
			.filter((el) => el.scrollTop > 0)
			.map(
				(el) =>
					`<${el.tagName.toLowerCase()} class="${el.getAttribute('class') ?? ''}"> scrollTop=${el.scrollTop} ` +
					`scrollHeight=${el.scrollHeight} clientHeight=${el.clientHeight}`
			)
	);
	expect(scrolled).toEqual([]);
});

test('390px: the open dropdown scrolls internally and keeps the New profile footer pinned', async ({ page }) => {
	await stub(page);
	const dialog = await openSpawn(page);

	await trigger(dialog).click();
	const pop = panel(page);
	await expect(pop).toBeVisible();
	await expect(trigger(dialog)).toHaveAttribute('aria-expanded', 'true');
	await expect(pop.getByRole('option')).toHaveCount(profiles.length);
	await expect(pop.locator('.rs-trailing, .rs-grip')).toHaveCount(0);
	await expect(pop.getByRole('option').locator('[data-journey="account-badge"]')).toHaveCount(
		profiles.length
	);
	await expect(pop.getByRole('option').locator('.name')).toHaveText(
		Array.from({ length: profiles.length }, (_, i) => `Profile ${i}`)
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

function near(a: number, b: number) {
	expect(Math.abs(a - b)).toBeLessThanOrEqual(1);
}

test('the closed card carries the badge and the cog inside it, and the open panel is attached at its width', async ({
	page
}) => {
	await stub(page, makeProfiles(3));
	const dialog = await openSpawn(page);
	const card = trigger(dialog);
	const cardBox = await box(card);
	await expect(card.locator('[data-journey="account-badge"]')).toBeVisible();
	const cog = dialog.getByRole('button', { name: 'Adjust profile' });
	const cogBox = await box(cog);
	expect(cogBox.x).toBeGreaterThan(cardBox.x);
	expect(cogBox.x + cogBox.width).toBeLessThan(cardBox.x + cardBox.width);
	expect(cogBox.y).toBeGreaterThanOrEqual(cardBox.y);
	expect(cogBox.y + cogBox.height).toBeLessThanOrEqual(cardBox.y + cardBox.height);

	await card.click();
	const pop = panel(page);
	await expect(pop).toBeVisible();
	await pop.evaluate((el) =>
		Promise.all(el.getAnimations({ subtree: true }).map((a) => a.finished.catch(() => undefined)))
	);
	const popBox = await box(pop);
	const openCard = await box(card);
	near(popBox.width, openCard.width);
	near(popBox.x, openCard.x);
	const side = await pop.getAttribute('data-side');
	if (side === 'top') near(popBox.y + popBox.height, openCard.y);
	else near(popBox.y, openCard.y + openCard.height);
});

test('the cog opens an edit form attached under the card at its width; the list closes it', async ({ page }) => {
	await stub(page, makeProfiles(3));
	const dialog = await openSpawn(page);
	await dialog.getByRole('button', { name: 'Adjust profile' }).click();
	const form = dialog.locator('[data-journey="profile-form"][data-mode="edit"]');
	await expect(form).toBeVisible();
	const cardBox = await box(trigger(dialog));
	const formBox = await box(form);
	near(formBox.width, cardBox.width);
	near(formBox.x, cardBox.x);
	near(formBox.y, cardBox.y + cardBox.height);

	await trigger(dialog).click();
	await expect(panel(page)).toBeVisible();
	await expect(form).toHaveCount(0);
});

test('New profile writes nothing until Create profile, and Cancel leaves the list unchanged', async ({ page }) => {
	const writes = await stub(page, makeProfiles(3));
	const dialog = await openSpawn(page);

	await trigger(dialog).click();
	await panel(page).locator('.rs-footer').getByRole('button', { name: 'New profile' }).click();
	const form = dialog.locator('[data-journey="profile-form"][data-mode="create"]');
	await expect(form).toBeVisible();
	await expect(dialog.locator('[data-journey="profile-draft"]')).toContainText('New profile');
	const create = form.getByRole('button', { name: 'Create profile' });
	await expect(create).toBeDisabled();
	await form.getByLabel('Profile name').fill('profile 1');
	await expect(create).toBeDisabled();
	await form.getByLabel('Profile name').fill('Reviewer');
	await expect(create).toBeEnabled();
	await form.getByRole('button', { name: 'Cancel' }).click();
	await expect(form).toHaveCount(0);
	await expect(trigger(dialog).locator('[data-profile-id="p0"]')).toBeVisible();
	expect(writes).toEqual([]);

	await trigger(dialog).click();
	await panel(page).locator('.rs-footer').getByRole('button', { name: 'New profile' }).click();
	await form.getByLabel('Profile name').fill('Reviewer');
	await form.getByRole('button', { name: 'Create profile' }).click();
	await expect.poll(() => writes).toEqual(['POST /api/v1/profiles']);
	await expect(form).toHaveCount(0);
});
