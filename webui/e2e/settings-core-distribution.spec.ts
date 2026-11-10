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

const CORE_SETTINGS_PAGES = [
	'appearance',
	'sessions',
	'execution',
	'privacy',
	'notifications',
	'events',
	'guides',
	'users',
	'dispatchers',
	'instance'
];

async function coreServer(page: Page, sessions: unknown[] = [session(1)], dispatchers: string[] = []) {
	await stubApi(page, sessions);
	await page.route('**/api/v1/sessions/dispatchers', (route) => route.fulfill({ json: dispatchers }));
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

test('a headless server shows the notice before login or workspace requests', async ({ page }) => {
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
	await expect(page.getByRole('heading', { name: 'Unsupported server' })).toBeVisible();
	await expect(page.getByText('This server has no web interface. Connect this client to a cctui server.')).toBeVisible();
	await expect(page.getByRole('link')).toHaveCount(0);
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
		await expect(page.getByRole('heading', { name: 'Unsupported server' })).toHaveCount(0);
	});
}

test('a server with every switchable feature off renders the app without the notice', async ({ page }) => {
	const errors = watch(page);
	const offRequests = await coreServer(page);
	await page.goto('/accounts');
	const nav = page.getByRole('navigation').first();
	await expect(nav.getByRole('link', { name: 'Accounts' })).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Unsupported server' })).toHaveCount(0);
	await expect(nav.getByRole('link', { name: 'Bookmarks' })).toHaveCount(0);
	await page.goto('/settings/dispatchers');
	await expect(page.locator('[data-settings-page="dispatchers"]')).toBeVisible();
	await expect(page.locator('[data-settings-page="plugins"]')).toHaveCount(0);
	await expect(page.locator('[data-settings-page="monitoring"]')).toHaveCount(0);
	expect(offRequests).toEqual([]);
	expect(errors).toEqual([]);
});

for (const hidden of ['/bookmarks', '/settings/plugins', '/settings/security', '/settings/macros', '/device']) {
	test(`a direct URL to ${hidden} is not found on a core-features server`, async ({ page }) => {
		const offRequests = await coreServer(page);
		await page.goto(hidden);
		await expect(page.getByText('Not Found')).toBeVisible();
		expect(offRequests).toEqual([]);
	});
}

test('the onboarding card on a core server asks for a dispatcher, then a dispatch', async ({ page }) => {
	const errors = watch(page);
	const offRequests = await coreServer(page, []);
	await page.goto('/sessions');
	const card = page.locator('[data-journey="first-run"]');
	await expect(card).toBeVisible();
	await expect(card.locator('[data-journey="first-run-step"]')).toHaveText([
		/Connect a provider account/,
		/Enroll a dispatcher/,
		/Dispatch a session/
	]);
	await expect(card.getByText('Enroll a machine')).toHaveCount(0);
	await expect(
		card.locator('[data-journey="first-run-start"][data-journey-key="dispatcher"]')
	).toHaveAttribute('href', '/settings/dispatchers');
	await card.locator('[data-journey="first-run-start"][data-journey-key="dispatch"]').click();
	await expect(page.getByRole('dialog').locator('#sp-prompt-d')).toBeVisible();
	expect(offRequests).toEqual([]);
	expect(errors).toEqual([]);
});

test('the guides page on a core server offers no machine or search guide', async ({ page }) => {
	const offRequests = await coreServer(page);
	await page.goto('/settings/guides');
	const guides = page.locator('[data-settings-page="guides"]');
	await expect(guides.locator('[data-journey="guide"][data-journey-key="welcome"]')).toBeVisible();
	for (const id of ['enroll-machine', 'spawn-session', 'search-sessions']) {
		await expect(guides.locator(`[data-journey="guide"][data-journey-key="${id}"]`)).toHaveCount(0);
	}
	expect(offRequests).toEqual([]);
});

test('the launch form on a core server is the dispatch form with no tab strip', async ({ page }) => {
	const errors = watch(page);
	const offRequests = await coreServer(page, [session(1)], ['k8s']);
	await page.goto('/sessions');
	await page.locator('[data-journey="new"]').click();
	const dialog = page.getByRole('dialog');
	await expect(dialog.locator('#sp-dispatcher')).toBeVisible();
	await expect(dialog.getByRole('radiogroup')).toHaveCount(0);
	await expect(dialog.locator('#sp-cwd')).toHaveCount(0);
	await expect(dialog.getByRole('button', { name: 'Dispatch', exact: true })).toBeVisible();
	await expect(dialog.getByRole('button', { name: 'Draft', exact: true })).toHaveCount(0);
	expect(offRequests).toEqual([]);
	expect(errors).toEqual([]);
});

test('Settings on a core server lists exactly the core pages', async ({ page }) => {
	const offRequests = await coreServer(page);
	await page.goto('/settings/appearance');
	const toc = page.locator('nav.toc [data-journey="settings-nav"]');
	await expect(toc.first()).toBeVisible();
	const pages = await toc.evaluateAll((els) => els.map((el) => el.getAttribute('data-journey-key')));
	expect(pages).toEqual(CORE_SETTINGS_PAGES);
	expect(offRequests).toEqual([]);
});
