import { expect, test } from '@playwright/test';
import { stubDrawer } from './drawer.fixture';

test.beforeEach(({ page }) => stubDrawer(page));

test('a picked look is painted again after a reload', async ({ page }) => {
	await page.goto('/settings/interface');
	await page.getByTestId('appearance-look').getByText('Depth', { exact: true }).click();
	await expect(page.locator('html')).toHaveAttribute('data-look', 'depth');

	await page.reload();
	await expect(page.locator('html')).toHaveAttribute('data-look', 'depth');
	const shell = await (await page.request.get('/settings/interface')).text();
	expect(shell).toContain('data-look');
});

test('the sound switch survives a reload and reveals the volume', async ({ page }) => {
	await page.goto('/settings/interface');
	const toggle = page.getByRole('switch', { name: 'Sound effects' });
	await expect(toggle).not.toBeChecked();
	await expect(page.getByRole('slider', { name: 'Volume' })).toHaveCount(0);
	await toggle.click();
	await expect(toggle).toBeChecked();
	await expect(page.getByRole('slider', { name: 'Volume' })).toBeVisible();

	await page.reload();
	await expect(page.getByRole('switch', { name: 'Sound effects' })).toBeChecked();
});

test('the motion switch turns every effect off and back on', async ({ page }) => {
	await page.goto('/settings/interface');
	const toggle = page.getByRole('switch', { name: 'Motion' });
	await expect(toggle).toBeChecked();
	await toggle.click();
	await expect(toggle).not.toBeChecked();
	await expect(page.locator('html')).toHaveAttribute('data-fx-focus', 'none');

	await page.reload();
	await expect(page.getByRole('switch', { name: 'Motion' })).not.toBeChecked();
	await page.getByRole('switch', { name: 'Motion' }).click();
	await expect(page.locator('html')).not.toHaveAttribute('data-fx-focus', 'none');
});
