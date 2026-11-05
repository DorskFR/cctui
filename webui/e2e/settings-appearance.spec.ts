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

test('the sound effects switch survives a reload', async ({ page }) => {
	await page.goto('/settings/interface');
	const toggle = page.getByTestId('appearance-sound').getByRole('switch', { name: 'Sound effects' });
	await expect(toggle).not.toBeChecked();
	await toggle.click();
	await expect(toggle).toBeChecked();

	await page.reload();
	await expect(
		page.getByTestId('appearance-sound').getByRole('switch', { name: 'Sound effects' })
	).toBeChecked();
});

test('the motion switch turns every effect off and back on', async ({ page }) => {
	await page.goto('/settings/interface');
	const toggle = page.getByTestId('appearance-motion').getByRole('switch');
	await expect(toggle).toBeChecked();
	await toggle.click();
	await expect(toggle).not.toBeChecked();
	await page.reload();
	await expect(page.getByTestId('appearance-motion').getByRole('switch')).not.toBeChecked();
	await page.getByTestId('appearance-motion').getByRole('switch').click();
	await expect(page.getByTestId('appearance-motion').getByRole('switch')).toBeChecked();
});
