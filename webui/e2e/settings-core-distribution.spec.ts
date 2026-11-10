import { expect, test, type Page } from '@playwright/test';
import { ROUTES } from '../src/lib/bindings/routes';
import { session, stubApi, watch } from './tiles.fixture';

const langfuse = { available: false, host: null, public_host: null, project_id: null };

/** Every feature that gates a route, plus `terminal`, which gates only a WebSocket message. */
const SWITCHABLE = [...new Set([...ROUTES.flatMap((r) => (r.feature ? [r.feature] : [])), 'terminal'])];

const MATCHERS = ROUTES.map((r) => ({
	method: r.method as string,
	gated: r.feature !== null,
	pattern: new RegExp(`^/api/v1${r.path.replace(/\{\*\w+\}/g, '.+').replace(/\{\w+\}/g, '[^/]+')}$`),
	literal: r.path.replace(/\{[^}]*\}/g, '').length
}));

/** The most literal matching route decides, as in the app's request gate. */
function isGated(method: string, path: string): boolean {
	let best: (typeof MATCHERS)[number] | null = null;
	for (const r of MATCHERS) {
		if (r.method !== method || !r.pattern.test(path)) continue;
		if (!best || r.literal > best.literal) best = r;
	}
	return best?.gated ?? false;
}

async function coreServer(page: Page) {
	await stubApi(page, [session(1)]);
	await page.route('**/api/v1/capabilities', (route) =>
		route.fulfill({
			json: { langfuse, distribution: 'full', workspace: true, experimental: false, features: [], switchable: SWITCHABLE }
		})
	);
	const offRequests: string[] = [];
	page.on('request', (req) => {
		const url = new URL(req.url());
		if (url.pathname.startsWith('/api/v1/') && isGated(req.method(), url.pathname)) {
			offRequests.push(`${req.method()} ${url.pathname}`);
		}
	});
	return offRequests;
}

test('the headless host shows the notice before login or workspace requests', async ({ page }) => {
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

test('a server with every switchable feature off renders the app without the notice', async ({ page }) => {
	const errors = watch(page);
	const offRequests = await coreServer(page);
	await page.goto('/accounts');
	const nav = page.getByRole('navigation').first();
	await expect(nav.getByRole('link', { name: 'Accounts' })).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Core server' })).toHaveCount(0);
	await expect(nav.getByRole('link', { name: 'Bookmarks' })).toHaveCount(0);
	await page.goto('/settings/dispatchers');
	await expect(page.locator('[data-settings-page="dispatchers"]')).toBeVisible();
	await expect(page.locator('[data-settings-page="plugins"]')).toHaveCount(0);
	await expect(page.locator('[data-settings-page="monitoring"]')).toHaveCount(0);
	expect(offRequests).toEqual([]);
	expect(errors).toEqual([]);
});

for (const hidden of ['/bookmarks', '/settings/plugins', '/settings/security', '/device']) {
	test(`a direct URL to ${hidden} is not found on a core-features server`, async ({ page }) => {
		const offRequests = await coreServer(page);
		await page.goto(hidden);
		await expect(page.getByText('Not Found')).toBeVisible();
		expect(offRequests).toEqual([]);
	});
}
