import { expect, test, type Page } from '@playwright/test';
import { ROUTES } from '../src/lib/bindings/routes';
import { ALL_FEATURES } from '../src/lib/features.fixture';
import { session, stubApi, watch } from './tiles.fixture';

const langfuse = { available: false, host: null, public_host: null, project_id: null };
const CORE = { langfuse, distribution: 'full', workspace: true, experimental: false, features: [], switchable: ALL_FEATURES };

const MATCHERS = ROUTES.map((r) => ({
	method: r.method as string,
	gated: r.feature !== null,
	pattern: new RegExp(`^/api/v1${r.path.replace(/\{\*\w+\}/g, '.+').replace(/\{\w+\}/g, '[^/]+')}$`),
	literal: r.path.replace(/\{[^}]*\}/g, '').length
}));

function isGated(method: string, path: string): boolean {
	let best: (typeof MATCHERS)[number] | null = null;
	for (const r of MATCHERS) {
		if (r.method !== method || !r.pattern.test(path)) continue;
		if (!best || r.literal > best.literal) best = r;
	}
	return best?.gated ?? false;
}

const live = session(1);
const archived = { ...session(2), status: 'archived', bucket: 'done' };

async function signedOutCoreServer(page: Page) {
	const state = { authed: false, capabilityAnswers: [] as number[], archiveBrowses: [] as string[] };
	await stubApi(page, [live]);
	await page.route('**/api/v1/**', async (route) => {
		const url = new URL(route.request().url());
		const path = url.pathname.replace('/api/v1', '');
		if (path === '/capabilities') {
			state.capabilityAnswers.push(state.authed ? 200 : 401);
			return state.authed ? route.fulfill({ json: CORE }) : route.fulfill({ status: 401, json: {} });
		}
		if (path === '/auth/login' && route.request().method() === 'POST') {
			state.authed = true;
			return route.fulfill({ json: {} });
		}
		if (path === '/auth/passkey/config') return route.fulfill({ status: 404, json: {} });
		if (!state.authed) return route.fulfill({ status: 401, json: {} });
		if (path === '/sessions/search') {
			const q = url.searchParams.get('q') ?? '';
			if (q) return route.fulfill({ status: 400, json: { error: 'transcript_search is off' } });
			state.archiveBrowses.push(url.search);
			return route.fulfill({ json: { sessions: [archived], total: 1 } });
		}
		return route.fallback();
	});
	const off = { beforeSignIn: [] as string[], afterSignIn: [] as string[] };
	page.on('request', (req) => {
		const { pathname } = new URL(req.url());
		if (pathname.startsWith('/api/v1/') && isGated(req.method(), pathname)) {
			(state.authed ? off.afterSignIn : off.beforeSignIn).push(`${req.method()} ${pathname}`);
		}
	});
	return { state, off };
}

test('signing in on a core server adopts its feature list without a reload', async ({ page }) => {
	const errors = watch(page);
	const { state, off } = await signedOutCoreServer(page);
	await page.goto('/sessions');

	const token = page.getByLabel('Token', { exact: true });
	await expect(token).toBeVisible();
	expect(state.capabilityAnswers).toEqual([401]);

	let reloaded = false;
	page.on('load', () => (reloaded = true));
	await token.fill('core-token');
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();

	const nav = page.getByRole('navigation', { name: 'Main navigation' }).first();
	await expect(nav.locator('[data-journey="nav"]').first()).toBeVisible();
	const keys = await nav
		.locator('[data-journey="nav"]')
		.evaluateAll((els) => els.map((el) => el.getAttribute('data-journey-key')));
	expect(keys).toEqual(['overview', 'sessions', 'accounts', 'settings']);
	expect(state.capabilityAnswers[0]).toBe(401);
	expect(state.capabilityAnswers).toContain(200);

	await expect(page.locator(`[data-session-id="${live.id}"]`).first()).toBeVisible();
	await expect(page.locator('[data-journey="search"]')).toHaveCount(0);
	await expect(page.locator('[data-journey="sections"]')).toBeVisible();

	await page.locator('[data-journey="sections"] button').first().click();
	await page.locator('[data-journey="option"][data-journey-key="archived"]').click();
	await page.keyboard.press('Escape');
	await expect(page.locator(`[data-session-id="${archived.id}"]`).first()).toBeVisible();
	expect(state.archiveBrowses.length).toBeGreaterThan(0);
	expect(state.archiveBrowses.every((search) => !new URLSearchParams(search).get('q'))).toBe(true);

	expect(reloaded).toBe(false);
	expect(off.beforeSignIn.every((r) => r === 'GET /api/v1/auth/passkey/config')).toBe(true);
	expect(off.afterSignIn).toEqual([]);
	expect(errors.filter((e) => !e.includes('Failed to load resource'))).toEqual([]);
});
