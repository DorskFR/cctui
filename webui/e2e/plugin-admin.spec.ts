import { expect, test, type Page } from '@playwright/test';
import { localToken } from '../scripts/local-token.mjs';

// Settings › Instance › Plugins (admin) and the user Settings › Plugins list,
// against a mocked admin plugin API: install from URL, instance toggle,
// uninstall, and the user-side rule that only instance-enabled plugins show
// with their settings form gated behind the personal switch.

const TOKEN: string = process.env.PLUGIN_E2E_TOKEN ?? localToken();

type AdminPlugin = {
	id: string;
	name: string;
	description: string;
	version: string;
	source: 'installed' | 'directory';
	enabled: boolean;
};

const userInfo = (p: AdminPlugin) => ({
	id: p.id,
	name: p.name,
	description: p.description,
	version: p.version,
	icon: 'eye',
	web: null,
	skills: [],
	enabled: false,
	settings: [{ key: 'host', label: 'Bind address', env: 'DEMO_HOST', type: 'string' }],
	config: {}
});

function mockAdminApi(page: Page, initial: AdminPlugin[]) {
	const state = { plugins: initial, calls: [] as string[] };
	const admin = '**/api/v1/admin/plugins';
	page.route(admin, async (route) => {
		const req = route.request();
		state.calls.push(`${req.method()} ${new URL(req.url()).pathname}`);
		if (req.method() === 'POST') {
			const body = req.postDataJSON() as { url: string };
			const installed: AdminPlugin = {
				id: 'fromurl',
				name: 'From URL',
				description: body.url,
				version: '2.0.0',
				source: 'installed',
				enabled: false
			};
			state.plugins = [...state.plugins.filter((p) => p.id !== installed.id), installed];
			return route.fulfill({ json: installed });
		}
		return route.fulfill({ json: state.plugins });
	});
	page.route(`${admin}/*`, async (route) => {
		const req = route.request();
		const id = new URL(req.url()).pathname.split('/').pop() ?? '';
		state.calls.push(`${req.method()} /api/v1/admin/plugins/${id}`);
		const plugin = state.plugins.find((p) => p.id === id);
		if (!plugin) return route.fulfill({ status: 404, json: { error: 'no installed plugin with that id' } });
		if (req.method() === 'DELETE') {
			state.plugins = state.plugins.filter((p) => p.id !== id);
			return route.fulfill({ status: 204, body: '' });
		}
		plugin.enabled = (req.postDataJSON() as { enabled: boolean }).enabled;
		return route.fulfill({ json: plugin });
	});
	page.route('**/api/v1/plugins', (route) =>
		route.fulfill({ json: state.plugins.filter((p) => p.enabled).map(userInfo) })
	);
	return state;
}

async function login(page: Page) {
	const res = await page.request.post('/api/v1/auth/login', { data: { token: TOKEN } });
	expect(res.ok(), `login failed with ${res.status()}`).toBe(true);
}

const row = (page: Page, id: string) => page.locator(`[data-journey="plugin-admin-row"][data-plugin="${id}"]`);

test.describe('instance plugins (admin)', () => {
	test.use({ viewport: { width: 1440, height: 900 } });

	test('lists sources, toggles instance-wide, installs from URL and uninstalls', async ({ page }) => {
		await login(page);
		const state = mockAdminApi(page, [
			{ id: 'demo', name: 'Demo', description: 'd', version: '0.0.1', source: 'installed', enabled: false },
			{ id: 'local', name: 'Local', description: 'l', version: '0.0.2', source: 'directory', enabled: true }
		]);
		await page.goto('/settings/instance');
		const group = page.locator('[data-journey="plugins-admin"]');
		await expect(group).toBeVisible();
		await expect(row(page, 'demo')).toContainText('0.0.1');
		await expect(row(page, 'demo')).toContainText('installed');
		await expect(row(page, 'local')).toContainText('from directory');
		await expect(row(page, 'local').locator('[data-journey="plugin-admin-switch"]')).toHaveCount(0);

		const sw = row(page, 'demo').locator('[data-journey="plugin-admin-switch"]');
		await expect(sw).not.toBeChecked();
		await sw.click();
		await expect(sw).toBeChecked();
		expect(state.calls).toContain('PATCH /api/v1/admin/plugins/demo');

		await page.locator('[data-journey="plugin-admin-url"]').fill('https://example.com/p.tgz');
		await page.locator('[data-journey="plugin-admin-install"]').click();
		await expect(row(page, 'fromurl')).toContainText('2.0.0');
		expect(state.calls).toContain('POST /api/v1/admin/plugins');

		page.once('dialog', (d) => d.accept());
		await row(page, 'fromurl').locator('[data-journey="plugin-admin-uninstall"]').click();
		await expect(row(page, 'fromurl')).toHaveCount(0);
		expect(state.calls).toContain('DELETE /api/v1/admin/plugins/fromurl');
	});

	test('users only see instance-enabled plugins, with the form behind their own switch', async ({ page }) => {
		await login(page);
		mockAdminApi(page, [
			{ id: 'demo', name: 'Demo', description: 'd', version: '0.0.1', source: 'installed', enabled: true },
			{ id: 'off', name: 'Off', description: 'o', version: '0.0.3', source: 'installed', enabled: false }
		]);
		const loaded = page.waitForResponse((r) => r.url().includes('/api/v1/settings') && r.request().method() === 'GET');
		await page.goto('/settings/plugins');
		await loaded;
		const sw = page.locator('[data-journey="plugin-switch"][data-plugin="demo"]');
		await expect(sw).toBeVisible();
		await expect(page.locator('[data-journey="plugin-switch"][data-plugin="off"]')).toHaveCount(0);
		if (await sw.isChecked()) await sw.click();
		await expect(sw).not.toBeChecked();
		await expect(page.locator('[data-journey="plugin-config"]')).toHaveCount(0);
		await sw.click();
		await expect(sw).toBeChecked();
		await expect(page.locator('[data-journey="plugin-config"]')).toHaveCount(1);
		await sw.click();
		await expect(sw).not.toBeChecked();
		await page.waitForTimeout(1500);
	});
});
