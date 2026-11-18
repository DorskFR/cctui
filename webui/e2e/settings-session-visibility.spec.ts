import { expect, test, type Page } from '@playwright/test';
import { ALL_FEATURES } from '../src/lib/features.fixture';
import { session, stubApi } from './tiles.fixture';
import { waitForDrawerOpen } from './drawer.fixture';

const BOB = { ...session(0), name: 'bob work', bucket: 'working', machine_id: 'm-bob', machine_name: 'workbench' };
const MINE = { ...session(1), name: 'my work', bucket: 'working', machine_id: 'm-me', machine_name: 'desk' };
const SESSIONS = [BOB, MINE];

// Keeps the Instance sections this spec does not stub from calling their endpoints.
const OFF = ['self_update', 'child_sessions', 'voice', 'plugins', 'uploads', 'profiles'];

const now = new Date().toISOString();
const userRow = (id: string, name: string) => ({
	id,
	name,
	created_at: now,
	revoked_at: null,
	disabled_at: null,
	can_dispatch: true,
	last_seen_at: null
});
const machineRow = (id: string, user_id: string, name: string) => ({
	id,
	user_id,
	name,
	display_name: null,
	first_seen_at: now,
	last_seen_at: now,
	revoked_at: null,
	kind: 'personal',
	hue: null,
	key_preview: null,
	liveness: 'online'
});
const MACHINES: Record<string, unknown[]> = {
	'u-me': [machineRow('m-me', 'u-me', 'desk')],
	'u-bob': [machineRow('m-bob', 'u-bob', 'workbench')]
};

interface Stub {
	hidden: Set<string>;
	writes: string[];
}

async function stubServer(page: Page, role: 'admin' | 'user'): Promise<Stub> {
	await stubApi(page, SESSIONS);
	const stub: Stub = { hidden: new Set(), writes: [] };
	const visible = (s: (typeof SESSIONS)[number]) =>
		!stub.hidden.has(`machine:${s.machine_id}`) && !(s.machine_id === 'm-bob' && stub.hidden.has('user:u-bob'));
	await page.route(
		(url) =>
			url.pathname === '/api/v1/me' ||
			url.pathname === '/api/v1/capabilities' ||
			url.pathname === '/api/v1/sessions' ||
			url.pathname === '/api/v1/admin/storage' ||
			url.pathname === '/api/v1/admin/instance/upstream-hosts' ||
			url.pathname === '/api/v1/admin/instance/event-retention' ||
			url.pathname === '/api/v1/admin/redteam' ||
			url.pathname.startsWith('/api/v1/me/hidden-owners') ||
			url.pathname.startsWith('/api/v1/admin/users'),
		async (route) => {
			const req = route.request();
			const path = new URL(req.url()).pathname.replace('/api/v1', '');
			if (path === '/me')
				return route.fulfill({
					json: {
						role,
						user_id: role === 'admin' ? 'u-me' : 'u-bob',
						user_name: role === 'admin' ? 'admin' : 'bob',
						machine_id: null,
						scopes: [],
						token_preview: 'cctui_u_test'
					}
				});
			if (path === '/capabilities')
				return route.fulfill({
					json: {
						langfuse: { available: false, host: null, public_host: null, project_id: null },
						distribution: 'full',
						workspace: true,
						experimental: false,
						features: ALL_FEATURES.filter((f) => !OFF.includes(f)),
						switchable: ALL_FEATURES
					}
				});
			if (path === '/admin/storage') return route.fulfill({ status: 403, json: { error: 'admin token required' } });
			if (path === '/admin/instance/upstream-hosts')
				return route.fulfill({ json: { hosts: [], source: 'default', env: [], managed: [] } });
			if (path === '/admin/instance/event-retention')
				return route.fulfill({ json: { days: 90, default_days: 90, source: 'default' } });
			if (path === '/admin/redteam') return route.fulfill({ json: { current_version: '0.32.0', versions: [] } });
			if (path === '/admin/users')
				return route.fulfill({ json: [userRow('u-me', 'admin'), userRow('u-bob', 'bob')] });
			const machines = path.match(/^\/admin\/users\/([^/]+)\/machines$/);
			if (machines) return route.fulfill({ json: MACHINES[machines[1]] ?? [] });
			if (path.startsWith('/admin/users')) return route.fulfill({ json: [] });
			if (path === '/me/hidden-owners')
				return route.fulfill({
					json: {
						owners: [...stub.hidden].map((key) => {
							const [kind, target_id] = key.split(':');
							return { kind, target_id, label: null };
						})
					}
				});
			const owner = path.match(/^\/me\/hidden-owners\/(user|machine)\/([^/]+)$/);
			if (owner) {
				const key = `${owner[1]}:${owner[2]}`;
				stub.writes.push(`${req.method()} ${key}`);
				if (req.method() === 'PUT') stub.hidden.add(key);
				if (req.method() === 'DELETE') stub.hidden.delete(key);
				return route.fulfill({ status: 204, body: '' });
			}
			return route.fulfill({
				json: { sessions: SESSIONS.filter(visible), parents: [], next_cursor: null, partial: false }
			});
		}
	);
	return stub;
}

const row = (page: Page, id: string) => page.locator(`[data-journey="session"][data-session-id="${id}"]`);
const bobSwitch = (page: Page) =>
	page.locator('[data-journey="visibility-user"][data-journey-key="u-bob"] [role="switch"]');

async function markDocument(page: Page) {
	await page.evaluate(() => {
		(window as unknown as { __sameDocument: boolean }).__sameDocument = true;
	});
}
const sameDocument = (page: Page) =>
	page.evaluate(() => (window as unknown as { __sameDocument?: boolean }).__sameDocument === true);

async function openInstance(page: Page) {
	await page.locator('[data-journey="nav"][data-journey-key="settings"]').first().click();
	await page.waitForURL(/\/settings\/interface/);
	await page.locator('nav.toc [data-journey="settings-nav"][data-journey-key="instance"]').click();
	await page.waitForURL(/\/settings\/instance/);
}

async function backToSessions(page: Page) {
	await page.locator('[data-journey="nav"][data-journey-key="sessions"]').first().click();
	await page.waitForURL(/\/sessions$/);
}

test.beforeEach(async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 900 });
});

test('an admin turns a user off: the list shrinks without a reload, a direct link still opens, on brings them back', async ({
	page
}) => {
	const stub = await stubServer(page, 'admin');
	await page.goto('/sessions');
	await expect(row(page, BOB.id)).toHaveCount(1);
	await expect(row(page, MINE.id)).toHaveCount(1);
	await expect(page.locator('[data-journey="show-hidden"]')).toHaveCount(0);
	await markDocument(page);

	await openInstance(page);
	await expect(bobSwitch(page)).toHaveAttribute('aria-checked', 'true');
	await bobSwitch(page).click();
	await expect.poll(() => stub.writes).toEqual(['PUT user:u-bob']);
	await expect(bobSwitch(page)).toHaveAttribute('aria-checked', 'false');
	await expect(
		page.locator('[data-journey="visibility-user"][data-journey-key="u-bob"] [data-journey="hidden-chip"]')
	).toBeVisible();

	await backToSessions(page);
	await expect(row(page, BOB.id)).toHaveCount(0);
	await expect(row(page, MINE.id)).toHaveCount(1);
	expect(await sameDocument(page)).toBe(true);

	await openInstance(page);
	await bobSwitch(page).click();
	await expect.poll(() => stub.writes).toEqual(['PUT user:u-bob', 'DELETE user:u-bob']);
	await backToSessions(page);
	await expect(row(page, BOB.id)).toHaveCount(1);
	expect(await sameDocument(page)).toBe(true);

	await openInstance(page);
	await bobSwitch(page).click();
	await expect.poll(() => stub.writes).toHaveLength(3);
	await page.goto(`/sessions/${BOB.id}`);
	await waitForDrawerOpen(page);
	await expect(page.locator('[data-journey="header"]').first()).toContainText('bob work');
	await expect(row(page, BOB.id)).toHaveCount(0);
});

test('a non-admin sees no visibility switch and no Instance page', async ({ page }) => {
	await stubServer(page, 'user');
	await page.goto('/settings/instance');
	await page.waitForURL(/\/settings\/accounts#profile$/);
	await expect(page.locator('nav.toc [data-journey="settings-nav"]').first()).toBeVisible();
	await expect(page.locator('nav.toc [data-journey="settings-nav"][data-journey-key="instance"]')).toHaveCount(0);
	await expect(page.locator('[data-visibility]')).toHaveCount(0);
	await expect(page.locator('[data-journey="visibility-user"]')).toHaveCount(0);
});
