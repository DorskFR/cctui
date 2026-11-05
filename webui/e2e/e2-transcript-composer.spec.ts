import { expect, test, type Page } from '@playwright/test';
import { stubDrawer, waitForDrawerOpen } from './drawer.fixture';

test.beforeEach(({ page }) => stubDrawer(page));

async function openDrawer(page: Page) {
	await page.setViewportSize({ width: 1440, height: 900 });
	await page.goto('/sessions');
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await waitForDrawerOpen(page);
}

test('line actions stay hidden until the line is hovered', async ({ page }) => {
	await openDrawer(page);
	const line = page.locator('[data-journey="line"]').nth(-2);
	await line.scrollIntoViewIfNeeded();
	const slot = line.locator('.actions-slot');
	await page.mouse.move(0, 0);
	await expect(slot).toHaveCSS('opacity', '0');
	await line.hover();
	await expect(slot).toHaveCSS('opacity', '1');
	const visible = slot.locator('button:not([popover] button)');
	expect(await visible.count()).toBeLessThanOrEqual(3);
});

test('role labels are plain text and times read HH:MM', async ({ page }) => {
	await openDrawer(page);
	const meta = page.locator('[data-journey="line"] .lmeta').first();
	await expect(meta.locator('[data-tsu="Badge"]')).toHaveCount(0);
	await expect(meta.locator('time.ltime')).toHaveText(/^\d{2}[:.h]\d{2}$/);
});

test('Send is outlined while the composer is empty and fills on focus', async ({ page }) => {
	await openDrawer(page);
	const split = page.locator('[data-journey="composer"] [data-tsu="SplitButton"]');
	await expect(split).not.toHaveClass(/split-primary/);
	await page.getByRole('textbox', { name: /message/i }).focus();
	await expect(split).toHaveClass(/split-primary/);
});
