import { expect, test } from '@playwright/test';
import { stubDrawer } from './drawer.fixture';

test.beforeEach(({ page }) => stubDrawer(page));

test('a picked look is painted again after a reload', async ({ page }) => {
	await page.goto('/settings/appearance');
	await page.getByTestId('appearance-look').getByText('Depth', { exact: true }).click();
	await expect(page.locator('html')).toHaveAttribute('data-look', 'depth');

	await page.reload();
	await expect(page.locator('html')).toHaveAttribute('data-look', 'depth');
	const shell = await (await page.request.get('/settings/appearance')).text();
	expect(shell).toContain('data-look');
});

test('the sound toggle survives a reload', async ({ page }) => {
	await page.goto('/settings/appearance');
	const sound = page.getByTestId('appearance-sound');
	await sound.getByRole('button').click();
	const toggle = page.getByRole('switch', { name: 'Sound effects' });
	await expect(toggle).not.toBeChecked();
	await toggle.click();
	await expect(toggle).toBeChecked();

	await page.reload();
	await page.getByTestId('appearance-sound').getByRole('button').click();
	await expect(page.getByRole('switch', { name: 'Sound effects' })).toBeChecked();
});
