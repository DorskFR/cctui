import { expect, test, type Page } from '@playwright/test';
import { session } from './tiles.fixture';

const ADMIN = { id: 'u-admin', name: 'admin' };
const BOB = { id: 'u-bob', name: 'bob' };

const user = (u: { id: string; name: string }) => ({
	...u,
	created_at: '2026-01-01T00:00:00Z',
	revoked_at: null,
	disabled_at: null,
	can_dispatch: true,
	last_seen_at: null
});
const machine = (id: string, owner: string, name: string) => ({
	id,
	user_id: owner,
	name,
	display_name: null,
	first_seen_at: '2026-01-01T00:00:00Z',
	last_seen_at: new Date().toISOString(),
	revoked_at: null,
	kind: 'persistent',
	liveness: 'online',
	hue: null,
	key_preview: null
});

const BOB_SESSION = { ...session(1), machine_id: 'm-bob', machine_name: 'bobbox', name: 'bob session' };
const MY_SESSION = { ...session(2), machine_id: 'm-admin', machine_name: 'adminbox', name: 'admin session' };

interface Stub {
	calls: string[];
	hidden: Set<string>;
}

async function stub(page: Page, role: 'admin' | 'user'): Promise<Stub> {
	const state: Stub = { calls: [], hidden: new Set() };
	const me = role === 'admin' ? ADMIN : BOB;
	await page.route('**/api/v1/**', async (route) => {
		const req = route.request();
		const path = new URL(req.url()).pathname.replace('/api/v1', '');
		const json = (body: unknown) => route.fulfill({ json: body });
		if (req.method() !== 'GET') state.calls.push(`${req.method()} ${path}`);
		if (path === '/me')
			return json({
				role,
				user_id: me.id,
				user_name: me.name,
				machine_id: null,
				scopes: role === 'admin' ? ['read', 'dispatch', 'enroll', 'admin'] : ['read', 'dispatch'],
				token_preview: 'cctui_u_ab…ef'
			});
		if (path === '/version') return json({ version: '0.30.0-beta.1' });
		if (path === '/settings') return json({ data: {} });
		if (path === '/drafts') return json({ drafts: [] });
		if (path === '/admin/users') return json([user(ADMIN), user(BOB)]);
		if (path === `/admin/users/${BOB.id}/machines`) return json([machine('m-bob', BOB.id, 'bobbox')]);
		if (path === `/admin/users/${ADMIN.id}/machines`) return json([machine('m-admin', ADMIN.id, 'adminbox')]);
		if (path === '/me/hidden-owners') {
			return json({
				owners: [...state.hidden].map((target_id) => ({ kind: 'user', target_id, label: 'bob' }))
			});
		}
		const hide = /^\/me\/hidden-owners\/user\/(.+)$/.exec(path);
		if (hide) {
			if (req.method() === 'PUT') state.hidden.add(hide[1]);
			if (req.method() === 'DELETE') state.hidden.delete(hide[1]);
			return route.fulfill({ status: 204 });
		}
		const listed = [BOB_SESSION, MY_SESSION].filter(
			(s) => !(s.machine_id === 'm-bob' && state.hidden.has(BOB.id))
		);
		if (path === '/sessions' || path.startsWith('/sessions/search'))
			return json({ sessions: listed, total: listed.length });
		const one = /^\/sessions\/([^/]+)$/.exec(path);
		if (one) return json([BOB_SESSION, MY_SESSION].find((s) => s.id === one[1]) ?? MY_SESSION);
		if (/^\/sessions\/[^/]+\/conversation/.test(path)) return json([]);
		return json([]);
	});
	await page.route('**/ws**', (r) => r.abort());
	return state;
}

test.describe('old settings URLs keep working', () => {
	test.beforeEach(async ({ page }) => {
		await stub(page, 'admin');
	});

	for (const [from, to] of [
		['/settings/appearance', /\/settings\/interface#theme$/],
		['/settings/security', /\/settings\/accounts#security$/],
		['/settings/users', /\/settings\/instance#users$/],
		['/settings/uploads', /\/settings\/instance#uploads$/],
		['/settings/events', /\/events$/],
		['/access', /\/settings\/instance#users$/],
		['/dispatchers', /\/settings\/instance#dispatchers$/],
		['/settings#passkeys', /\/settings\/accounts#security$/],
		['/settings', /\/settings\/interface$/]
	] as const) {
		test(`${from} lands on its new location`, async ({ page }) => {
			await page.goto(from);
			await expect(page).toHaveURL(to);
		});
	}

	test('/events is a page of its own', async ({ page }) => {
		await page.goto('/events');
		await expect(page).toHaveURL(/\/events$/);
		await expect(page.locator('[data-journey="page"][data-journey-key="events"]')).toBeVisible();
	});
});

test.describe('the settings areas', () => {
	test('admins see five areas plus Instance', async ({ page }) => {
		await stub(page, 'admin');
		await page.setViewportSize({ width: 1440, height: 900 });
		await page.goto('/settings/interface');
		const keys = await page
			.locator('.toc [data-journey="settings-nav"]')
			.evaluateAll((els) => els.map((e) => e.getAttribute('data-journey-key')));
		expect(keys).toEqual(['interface', 'accounts', 'sessions', 'notifications', 'plugins', 'instance']);
	});

	test('non-admins see no Instance area and no visibility switch', async ({ page }) => {
		await stub(page, 'user');
		await page.setViewportSize({ width: 1440, height: 900 });
		await page.goto('/settings/instance');
		await expect(page).toHaveURL(/\/settings\/interface$/);
		await expect(page.locator('.toc [data-journey="settings-nav"][data-journey-key="instance"]')).toHaveCount(0);
		await expect(page.locator('[data-journey="user-visibility"]')).toHaveCount(0);
	});
});

test.describe('show sessions in my list', () => {
	test.use({ viewport: { width: 1440, height: 900 } });

	test('an admin turns a user off: the list shrinks, a direct link still opens', async ({ page }) => {
		const state = await stub(page, 'admin');
		await page.goto('/sessions');
		await expect(page.getByText('bob session').first()).toBeVisible();

		await page.goto('/settings/instance#users');
		const sw = page.locator(`[data-journey="user-visibility"][data-journey-key="${BOB.id}"]`).first();
		await expect(sw).toBeChecked();
		await sw.click();
		await expect(sw).not.toBeChecked();
		expect(state.calls).toContain(`PUT /me/hidden-owners/user/${BOB.id}`);
		await expect(page.locator('[data-journey="hidden-chip"]').first()).toBeVisible();
		await expect(page.locator(`[data-journey="user-visibility"][data-journey-key="${ADMIN.id}"]`)).toHaveCount(0);

		await page.goto('/sessions');
		await expect(page.getByText('admin session').first()).toBeVisible();
		await expect(page.getByText('bob session')).toHaveCount(0);

		await page.goto(`/sessions/${BOB_SESSION.id}`);
		await expect(page.getByText('bob session').first()).toBeVisible();

		await page.goto('/settings/instance#users');
		const back = page.locator(`[data-journey="user-visibility"][data-journey-key="${BOB.id}"]`).first();
		await back.click();
		await expect(back).toBeChecked();
		expect(state.calls).toContain(`DELETE /me/hidden-owners/user/${BOB.id}`);
		await page.goto('/sessions');
		await expect(page.getByText('bob session').first()).toBeVisible();
	});
});
