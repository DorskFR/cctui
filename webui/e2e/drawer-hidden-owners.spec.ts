import { expect, test, type Page } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';
import { waitForDrawerOpen } from './drawer.fixture';

const BOB = { ...session(0), name: 'bob work', bucket: 'working', machine_id: 'm-bob', machine_name: 'workbench' };
const MINE = { ...session(1), name: 'my work', bucket: 'working', machine_id: 'm-me', machine_name: 'desk' };
const SESSIONS = [BOB, MINE];

const machineRow = (id: string, user_id: string, name: string) => ({
	id,
	user_id,
	name,
	display_name: null,
	first_seen_at: new Date().toISOString(),
	last_seen_at: new Date().toISOString(),
	revoked_at: null,
	kind: 'personal',
	hue: null,
	key_preview: null,
	liveness: 'online'
});
const userRow = (id: string, name: string) => ({
	id,
	name,
	created_at: new Date().toISOString(),
	revoked_at: null,
	disabled_at: null,
	can_dispatch: true,
	last_seen_at: null
});
const MACHINES: Record<string, unknown[]> = {
	'u-me': [machineRow('m-me', 'u-me', 'desk')],
	'u-bob': [machineRow('m-bob', 'u-bob', 'workbench')]
};
const LABELS: Record<string, string> = { 'machine:m-bob': 'workbench', 'machine:m-me': 'desk', 'user:u-bob': 'bob' };

interface Stub {
	hidden: Set<string>;
	writes: string[];
	listShowHidden: boolean[];
}

async function stubHiddenOwners(page: Page): Promise<Stub> {
	await stubApi(page, SESSIONS);
	const stub: Stub = { hidden: new Set(), writes: [], listShowHidden: [] };
	const isHidden = (s: (typeof SESSIONS)[number]) =>
		stub.hidden.has(`machine:${s.machine_id}`) ||
		(s.machine_id === 'm-bob' && stub.hidden.has('user:u-bob'));
	await page.route(
		(url) =>
			url.pathname === '/api/v1/me' ||
			url.pathname.startsWith('/api/v1/me/hidden-owners') ||
			url.pathname.startsWith('/api/v1/admin/users') ||
			url.pathname === '/api/v1/sessions',
		async (route) => {
			const req = route.request();
			const url = new URL(req.url());
			const path = url.pathname.replace('/api/v1', '');
			if (path === '/me')
				return route.fulfill({
					json: {
						role: 'admin',
						user_id: 'u-me',
						user_name: 'admin',
						machine_id: null,
						scopes: [],
						token_preview: 'cctui_u_test'
					}
				});
			if (path === '/admin/users') return route.fulfill({ json: [userRow('u-me', 'admin'), userRow('u-bob', 'bob')] });
			const machines = path.match(/^\/admin\/users\/([^/]+)\/machines$/);
			if (machines) return route.fulfill({ json: MACHINES[machines[1]] ?? [] });
			if (path.startsWith('/admin/users')) return route.fulfill({ json: [] });
			if (path === '/me/hidden-owners')
				return route.fulfill({
					json: {
						owners: [...stub.hidden].map((key) => {
							const [kind, target_id] = key.split(':');
							return { kind, target_id, label: LABELS[key] ?? null };
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
			const showHidden = url.searchParams.get('show_hidden') === 'true';
			stub.listShowHidden.push(showHidden);
			const sessions = SESSIONS.flatMap((s) =>
				!isHidden(s) ? [s] : showHidden ? [{ ...s, hidden: true }] : []
			);
			return route.fulfill({ json: { sessions, parents: [], next_cursor: null, partial: false } });
		}
	);
	await page.route('**/ws**', (route) => route.abort());
	return stub;
}

const row = (page: Page, id: string) => page.locator(`[data-journey="session"][data-session-id="${id}"]`);
const chip = (page: Page, id: string) => row(page, id).locator('[data-journey="hidden-chip"]');
const showHidden = (page: Page) => page.locator('[data-journey="show-hidden"]');

async function openMenu(page: Page, id: string) {
	await row(page, id).locator('[data-journey="title"]').click();
	await waitForDrawerOpen(page);
	await page.locator('[data-journey="header"] [data-journey="actions"]').click();
}

test.beforeEach(async ({ page }) => {
	await page.setViewportSize({ width: 1280, height: 900 });
});

test('hide a machine, see it under Show hidden with the chip, unhide it', async ({ page }) => {
	const stub = await stubHiddenOwners(page);
	await page.goto('/sessions');
	await expect(row(page, BOB.id)).toHaveCount(1);
	await expect(row(page, MINE.id)).toHaveCount(1);

	await openMenu(page, BOB.id);
	await expect(page.getByRole('menuitem', { name: 'Hide sessions from user bob' })).toBeVisible();
	await page.getByRole('menuitem', { name: 'Hide sessions from machine workbench' }).click();

	await expect.poll(() => stub.writes).toEqual(['PUT machine:m-bob']);
	await expect(row(page, BOB.id)).toHaveCount(0);
	await expect(row(page, MINE.id)).toHaveCount(1);

	await page.goto('/sessions');
	await expect(row(page, MINE.id)).toHaveCount(1);
	await expect(row(page, BOB.id)).toHaveCount(0);
	await showHidden(page).click();
	await expect(showHidden(page)).toHaveAttribute('aria-pressed', 'true');
	await expect(row(page, BOB.id)).toHaveCount(1);
	await expect(chip(page, BOB.id)).toHaveText('hidden');
	await expect(chip(page, MINE.id)).toHaveCount(0);
	expect(stub.listShowHidden).toContain(true);

	await openMenu(page, BOB.id);
	await page.getByRole('menuitem', { name: 'Show sessions from machine workbench' }).click();
	await expect.poll(() => stub.writes).toEqual(['PUT machine:m-bob', 'DELETE machine:m-bob']);
	await expect(chip(page, BOB.id)).toHaveCount(0);

	await page.goto('/sessions');
	await expect(row(page, BOB.id)).toHaveCount(1);
	await expect(chip(page, BOB.id)).toHaveCount(0);
});

test('the admin’s own user is never offered for hiding, only their machine', async ({ page }) => {
	await stubHiddenOwners(page);
	await page.goto('/sessions');
	await openMenu(page, MINE.id);
	await expect(page.getByRole('menuitem', { name: 'Hide sessions from machine desk' })).toBeVisible();
	await expect(page.getByRole('menuitem', { name: /Hide sessions from user/ })).toHaveCount(0);
});
