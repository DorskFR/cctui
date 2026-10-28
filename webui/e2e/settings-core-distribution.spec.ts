import { expect, test } from '@playwright/test';

const langfuse = { available: false, host: null, public_host: null, project_id: null };

test('core explains workspace availability before login or workspace requests', async ({ page }) => {
	const requests: string[] = [];
	const sockets: string[] = [];
	page.on('websocket', (socket) => sockets.push(socket.url()));
	await page.route('**/api/v1/**', async (route) => {
		const path = new URL(route.request().url()).pathname;
		requests.push(path);
		if (path === '/api/v1/capabilities') {
			await route.fulfill({ json: { langfuse, distribution: 'core', workspace: false } });
		} else {
			await route.fulfill({ status: 401, json: {} });
		}
	});
	await page.goto('/sessions');
	await expect(page.getByRole('heading', { name: 'Core server' })).toBeVisible();
	await expect(page.getByText('Use the execution API, or connect this client to a full cctui server.')).toBeVisible();
	await expect(page.getByLabel('Token', { exact: true })).toHaveCount(0);
	expect(requests).toEqual(['/api/v1/capabilities']);
	expect(sockets).toEqual([]);
});

for (const legacy of ['flags absent', 'endpoint absent']) {
	test(`older full servers still show login with ${legacy}`, async ({ page }) => {
		await page.route('**/api/v1/**', async (route) => {
			if (new URL(route.request().url()).pathname === '/api/v1/capabilities') {
				await route.fulfill({ status: legacy === 'endpoint absent' ? 404 : 200, json: { langfuse } });
			} else {
				await route.fulfill({ status: 401, json: {} });
			}
		});
		await page.goto('/sessions');
		await expect(page.getByLabel('Token', { exact: true })).toBeVisible();
		await expect(page.getByRole('heading', { name: 'Core server' })).toHaveCount(0);
	});
}
