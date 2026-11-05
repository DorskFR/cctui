import { expect, test } from '@playwright/test';
import { NARROW, WIDE, openDrawer, pageOverflow } from './drawer-header.helpers';

const header = '[data-journey="header"]';
const bar = `${header} .dbar [data-tsu="Toolbar"]`;

test('an idle session shows no Stop, and no rename, archive or text size in the bar', async ({ page }) => {
	await openDrawer(page, WIDE);
	await expect(page.locator(`${bar} [data-journey="stop"]`)).toHaveCount(0);
	await expect(page.locator(`${bar} [data-tsu="FontScalePicker"]`)).toHaveCount(0);
	await expect(page.locator(`${bar} button[aria-label="Rename"], ${bar} button[aria-label="Archive"]`)).toHaveCount(0);
});

test('the ⋯ menu is grouped by separators and holds rename, text size and archive', async ({ page }) => {
	await openDrawer(page, WIDE);
	await page.locator(`${header} [aria-haspopup="menu"]`).click();
	const menu = page.locator('[role="menu"]').last();
	await expect(menu).toBeVisible();
	expect(await menu.locator('[data-menu-sep]').count()).toBeGreaterThanOrEqual(2);
	await expect(menu.getByRole('menuitem', { name: 'Copy link' })).toBeVisible();
	await expect(menu.getByRole('menuitem', { name: /Rename/ })).toBeVisible();
	await expect(menu.getByRole('menuitem', { name: /Archive/ })).toBeVisible();
	await expect(menu.locator('[data-tsu="FontScalePicker"]')).toHaveCount(1);
	await expect(menu.getByRole('menuitem', { name: /Hide sessions from/ })).toHaveCount(0);
});

test('the overlay drawer dims the page behind it', async ({ page }) => {
	await openDrawer(page, WIDE);
	const scrim = page.locator('[data-tsu="Scrim"]');
	await expect(scrim).toHaveCount(1);
	const bg = await scrim.evaluate((el) => getComputedStyle(el).backgroundColor);
	expect(bg).toMatch(/rgba?\(0, 0, 0, 0\.4\)|color\(srgb 0 0 0 \/ 0\.4\)/);
	await scrim.click({ position: { x: 10, y: 10 } });
	await expect(page.locator(header)).toHaveCount(0);
});

test('the scrim sits under the drawer: its own controls take the clicks', async ({ page }) => {
	await openDrawer(page, { width: 1440, height: 900 });
	await expect(page.locator('[data-tsu="Scrim"]')).toHaveCount(1);
	const details = page.locator('[data-journey="head-details"]');
	const box = (await details.boundingBox())!;
	const hit = await page.evaluate(
		({ x, y }) => {
			const el = document.elementFromPoint(x, y);
			return {
				inPanel: !!el?.closest('[data-tsu="ResizablePanel"] .panel'),
				scrim: !!el?.closest('[data-tsu="Scrim"]')
			};
		},
		{ x: box.x + box.width / 2, y: box.y + box.height / 2 }
	);
	expect(hit).toEqual({ inPanel: true, scrim: false });
	await details.click();
	await expect(page.locator('.metapop')).toBeVisible();
	await page.keyboard.press('Escape');
	await page.locator('[data-journey="header"] [aria-haspopup="menu"]').click();
	await expect(page.locator('[role="menu"]').last()).toBeVisible();
	await expect(page.locator('[data-journey="header"]')).toHaveCount(1);
});

test('auto-approve is a switch that says its state', async ({ page }) => {
	await openDrawer(page, WIDE);
	const auto = page.locator('[data-journey="auto-approve"]');
	await expect(auto.getByRole('switch')).toHaveAttribute('aria-checked', /true|false/);
	await expect(auto).toContainText(/Auto-approve: (on|off)/);
});

test('the meta row spells its token total and shows no $0.00', async ({ page }) => {
	await openDrawer(page, WIDE);
	const meta = page.locator('[data-journey="head-meta"]');
	await expect(meta).not.toContainText('Σ');
	await expect(meta).not.toContainText('$0.00');
	const tokens = meta.locator('.tokens');
	if (await tokens.count()) await expect(tokens).toContainText(/tokens?/);
});

test(`${NARROW.width}px: the title gets its own line and the bar keeps at most two icons`, async ({
	page
}) => {
	await openDrawer(page, NARROW);
	const head = page.locator(header);
	const title = head.locator('.dtitle');
	const barBox = (await head.locator('.dbar').boundingBox())!;
	const titleBox = (await title.boundingBox())!;
	expect(titleBox.y).toBeGreaterThanOrEqual(barBox.y + barBox.height - 1);
	const icons = page.locator(
		`${bar} > button:visible:not([data-journey="stop"]), ${bar} > [data-tsu="Popover"]:visible`
	);
	expect(await icons.count()).toBeLessThanOrEqual(2);
	await expect(page.locator('[data-tsu="Scrim"]')).toHaveCount(0);
	expect(await pageOverflow(page)).toBeLessThanOrEqual(1);
});
