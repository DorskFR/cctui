import { expect, test, type Page } from '@playwright/test';
import { existsSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'vite';
import { cctuiPluginConfig } from '../plugin-sdk/vite';
import { localToken } from '../scripts/local-token.mjs';

// Runtime plugins end to end: a demo plugin is built with the SDK helper into a
// temp plugins dir and served under /plugins/ by request interception, along
// with GET /api/v1/plugins (the server half is mocked; the webui is real).

const TOKEN: string = process.env.PLUGIN_E2E_TOKEN ?? localToken();
const SHOTS = process.env.PLUGIN_E2E_SHOTS ?? 'test-results/plugin-review-pane';
const webui = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const fixture = join(webui, 'e2e/fixtures/demo-plugin');
const pluginsDir = join(tmpdir(), `cctui-e2e-plugins-${process.pid}`);
const webDir = join(pluginsDir, 'demo/web');
const WEB_URL = '/plugins/demo/web/index.js?v=e2e00001';

let bundle = '';

test.beforeAll(async () => {
	await build({
		configFile: false,
		root: fixture,
		logLevel: 'error',
		...cctuiPluginConfig({ entry: join(fixture, 'src/index.ts'), outDir: webDir })
	});
	expect(existsSync(join(webDir, 'index.js'))).toBe(true);
	bundle = readFileSync(join(webDir, 'index.js'), 'utf8');
	expect(bundle).toContain('from "/plugin-runtime/svelte-internal-client.js"');
	expect(bundle).toContain('from "/plugin-runtime/tsumikit.js"');
});

const pluginInfo = (enabled: boolean) => [
	{
		id: 'demo',
		name: 'Demo',
		description: 'E2E fixture built with the plugin SDK.',
		version: '0.0.1',
		icon: 'eye',
		web: WEB_URL,
		skills: [],
		enabled,
		settings: [{ key: 'host', label: 'Bind address', env: 'DEMO_HOST', type: 'string' }],
		config: {}
	}
];

async function serveFakePluginServer(page: Page, enabled: boolean) {
	await page.route('**/api/v1/plugins', (route) => route.fulfill({ json: pluginInfo(enabled) }));
	await page.route('**/plugins/demo/web/index.js*', (route) =>
		route.fulfill({ body: bundle, contentType: 'text/javascript', headers: { 'cache-control': 'no-cache' } })
	);
}

async function login(page: Page) {
	const res = await page.request.post('/api/v1/auth/login', { data: { token: TOKEN } });
	expect(res.ok(), `login failed with ${res.status()}`).toBe(true);
}

async function setPlugin(page: Page, on: boolean) {
	const loaded = page.waitForResponse((r) => r.url().includes('/api/v1/settings') && r.request().method() === 'GET');
	await page.goto('/settings/plugins');
	await loaded;
	const sw = page.locator('[data-journey="plugin-switch"][data-plugin="demo"]');
	await expect(sw).toBeVisible();
	if ((await sw.isChecked()) !== on) await sw.click();
	await expect(sw).toBeChecked({ checked: on });
	await page.waitForTimeout(1500);
}

async function openDrawer(page: Page) {
	await page.goto('/sessions');
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
}

const pluginButton = (page: Page) => page.locator('[data-journey="plugin"][data-plugin="demo"]');
const pane = (page: Page) => page.locator('[data-journey="demo-pane"]');
const composerText = (page: Page) => page.locator('[data-journey="message"]');

/** Console/page errors plus every failed response; the local stack's own
 *  404s (fixtures without attachments, etc.) are reported, not asserted. */
function collectErrors(page: Page): string[] {
	const errors: string[] = [];
	page.on('console', (msg) => {
		if (msg.type() === 'error' && !/Failed to load resource/.test(msg.text())) errors.push(msg.text());
	});
	page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
	page.on('response', (r) => {
		if (r.status() >= 400) errors.push(`http ${r.status()} ${new URL(r.url()).pathname}`);
	});
	return errors;
}
function relevant(errors: string[]): string[] {
	const noise = errors.filter((e) => /^http \d+ /.test(e) && !/\/plugins\/|\/plugin-runtime\//.test(e));
	if (noise.length) console.log(`unrelated failed responses: ${[...new Set(noise)].join(', ')}`);
	return errors.filter((e) => !noise.includes(e) && !/WebSocket|ws:\/\/|wss:\/\//.test(e));
}

async function shootNarrow(page: Page, name: string, fullPage = false) {
	await page.setViewportSize({ width: 420, height: 860 });
	await page.screenshot({ path: `${SHOTS}/${name}`, fullPage });
	await page.setViewportSize({ width: 1440, height: 900 });
}

test.describe.configure({ mode: 'serial' });

test.describe('runtime plugins', () => {
	test.use({ viewport: { width: 1440, height: 900 } });

	test('Settings › Plugins explains CCTUI_PLUGINS_DIR when nothing is installed', async ({ page }) => {
		await login(page);
		await page.route('**/api/v1/plugins', (route) => route.fulfill({ json: [] }));
		await page.goto('/settings/plugins');
		await expect(page.locator('[data-journey="plugins-empty"]')).toContainText('CCTUI_PLUGINS_DIR');
		await expect(page.locator('[data-journey="plugin-switch"]')).toHaveCount(0);
	});

	test('disabled by default: no button, no message action, nothing imported', async ({ page }) => {
		await login(page);
		await serveFakePluginServer(page, false);
		await setPlugin(page, false);
		const imported: string[] = [];
		page.on('request', (r) => {
			if (r.url().includes('/plugins/demo/')) imported.push(r.url());
		});
		await openDrawer(page);
		await expect(page.locator('[data-journey="line"][data-journey-key="assistant"]').first()).toBeVisible();
		await expect(pluginButton(page)).toHaveCount(0);
		await expect(page.locator('[data-journey="plugin-action"]')).toHaveCount(0);
		expect(imported).toEqual([]);
	});

	test('enabling it lists the plugin, adds the drawer button and the message actions', async ({ page }) => {
		const errors = collectErrors(page);
		await login(page);
		await serveFakePluginServer(page, false);
		await setPlugin(page, true);
		await page.screenshot({ path: `${SHOTS}/settings-plugins.png`, fullPage: true });
		await shootNarrow(page, 'settings-plugins-420.png', true);
		await openDrawer(page);
		await expect(pluginButton(page).first()).toBeVisible();
		await expect(page.locator('[data-journey="plugin-action"][data-plugin="demo"]').first()).toBeVisible();
		expect(relevant(errors)).toEqual([]);
	});

	test('the newest matching line auto-opens the pane once; closing it stays closed', async ({ page }) => {
		const errors = collectErrors(page);
		await login(page);
		await serveFakePluginServer(page, true);
		await openDrawer(page);
		await expect(pane(page)).toBeVisible();
		await expect(pane(page)).toHaveAttribute('data-word', /test/i);
		await pane(page).locator('[data-journey="demo-close"]').click();
		await expect(pane(page)).toHaveCount(0);
		await page.waitForTimeout(500);
		await expect(pane(page)).toHaveCount(0);
		expect(relevant(errors)).toEqual([]);
	});

	test('a message action opens the pane with its params on the shared runtime', async ({ page }) => {
		const errors = collectErrors(page);
		await login(page);
		await serveFakePluginServer(page, true);
		await openDrawer(page);
		if (await pane(page).count()) await pane(page).locator('[data-journey="demo-close"]').click();
		const action = page.locator('[data-journey="plugin-action"][data-plugin="demo"]').first();
		await action.scrollIntoViewIfNeeded();
		await action.click();
		await expect(pane(page)).toBeVisible();
		await expect(pane(page)).toHaveAttribute('data-word', /test/i);
		// Host context reaches the plugin only if both share one Svelte runtime.
		await expect(pane(page)).toHaveAttribute('data-host-origin', new URL(page.url()).origin);
		await expect(pane(page).locator('[data-journey="demo-session"]')).not.toBeEmpty();
		const count = pane(page).locator('[data-journey="demo-count"]');
		await count.click();
		await count.click();
		await expect(count).toHaveText('count 2');
		const bg = await count.evaluate((el) => getComputedStyle(el).backgroundColor);
		expect(bg).not.toBe('rgba(0, 0, 0, 0)');
		await composerText(page).fill('');
		await pane(page).locator('[data-journey="demo-insert"]').click();
		await expect(composerText(page)).toHaveValue(/\[demo\] test/i);
		await composerText(page).fill('');
		await page.screenshot({ path: `${SHOTS}/message-action-pane.png` });
		await page.setViewportSize({ width: 420, height: 860 });
		await openDrawer(page);
		await expect(pane(page)).toBeVisible();
		await pane(page).locator('[data-journey="demo-close"]').click();
		await expect(pane(page)).toBeHidden();
		await action.scrollIntoViewIfNeeded();
		await action.click();
		await expect(pane(page)).toBeVisible();
		await page.screenshot({ path: `${SHOTS}/message-action-pane-420.png` });
		await page.setViewportSize({ width: 1440, height: 900 });
		expect(relevant(errors)).toEqual([]);
	});

	test('a bundle targeting another cctuiApi is refused', async ({ page }) => {
		await login(page);
		await serveFakePluginServer(page, true);
		await page.route('**/plugins/demo/web/index.js*', (route) =>
			route.fulfill({ body: 'export default { cctuiApi: 2 };', contentType: 'text/javascript' })
		);
		await openDrawer(page);
		await expect(page.locator('[data-journey="line"][data-journey-key="assistant"]').first()).toBeVisible();
		await expect(pluginButton(page)).toHaveCount(0);
		await expect(page.locator('[data-journey="plugin-action"]')).toHaveCount(0);
	});

	test('disabling it removes everything again', async ({ page }) => {
		await login(page);
		await serveFakePluginServer(page, true);
		await setPlugin(page, false);
		await openDrawer(page);
		await expect(pluginButton(page)).toHaveCount(0);
		await expect(page.locator('[data-journey="plugin-action"]')).toHaveCount(0);
	});
});
