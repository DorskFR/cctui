import { expect, test, type Page } from '@playwright/test';

// The whole API is stubbed: this reproduces a client-side crash, so the only
// thing that must be real is the app bundle.
const N = Number(process.env.TILES_N ?? 24);

function session(i: number) {
	const now = new Date(Date.now() - i * 60_000).toISOString();
	return {
		id: `00000000-0000-4000-8000-${String(i).padStart(12, '0')}`,
		parent_id: null,
		machine_id: 'm1',
		machine_name: 'workbench',
		machine_kind: 'personal',
		working_dir: `/home/dorsk/Documents/proj-${i}`,
		status: 'active',
		liveness: 'online',
		bucket: i % 3 === 0 ? 'working' : i % 3 === 1 ? 'blocked' : 'done',
		token_usage: { input: 0, output: 0, cache_read: 0, cache_write: 0, total: 0 },
		metadata: {},
		adapter_id: 'claude_code',
		name: `session ${i}`,
		model: 'claude-opus-5',
		auto_approve: false,
		cache_cold: false,
		hibernated: false,
		pinned: false,
		labels: [],
		unread_count: 0,
		tool_use_count: 0,
		todos: [],
		has_token_credentials: true,
		account_traffic_observed: true,
		registered_at: now,
		last_activity_at: now,
		last_heartbeat: now
	};
}

const SESSIONS = Array.from({ length: N }, (_, i) => session(i));

async function stubApi(page: Page, seen: Set<string>) {
	await page.route('**/api/v1/**', async (route) => {
		const path = new URL(route.request().url()).pathname.replace('/api/v1', '');
		const json = (body: unknown) => route.fulfill({ json: body });
		if (path === '/me') return json({ id: 'u1', name: 'dorsk' });
		if (path === '/version') return json({ version: '0.23.0-beta.10' });
		if (path === '/settings') return json({ data: {} });
		if (path === '/labels') return json({ labels: [] });
		if (path.endsWith('/user-actions')) return json({ session_id: path.split('/')[2], items: [] });
		if (path === '/sessions' || path.startsWith('/sessions/search'))
			return json({ sessions: SESSIONS, total: SESSIONS.length });
		if (/^\/sessions\/[^/]+\/conversation/.test(path)) {
			seen.add(path);
			return json([]);
		}
		if (/^\/sessions\/[^/]+$/.test(path)) {
			const id = path.slice('/sessions/'.length);
			return json(SESSIONS.find((s) => s.id === id) ?? SESSIONS[0]);
		}
		return json([]);
	});
	// The socket is not what we are testing; let it fail closed.
	await page.route('**/ws**', (route) => route.abort());
}

function watch(page: Page) {
	const errors: string[] = [];
	page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
	page.on('console', (msg) => {
		// A stubbed run has no auth cookie, so the socket upgrade always fails.
		if (msg.type() === 'error' && !msg.text().includes('WebSocket connection to')) {
			errors.push(`console.error: ${msg.text()}`);
		}
	});
	return errors;
}

const metrics = (page: Page) =>
	page.evaluate(() => {
		const main = document.querySelector('main.content') as HTMLElement | null;
		const grid = document.querySelector('[data-journey="session-tiles"]') as HTMLElement | null;
		return {
			mainH: main?.clientHeight ?? 0,
			gridW: grid?.clientWidth ?? 0,
			gridH: grid?.clientHeight ?? 0,
			panes: document.querySelectorAll('[data-journey="session-tiles"] .tile').length,
			docScrollH: document.documentElement.scrollHeight,
			winH: window.innerHeight,
			view: localStorage.getItem('cctui_list_view')
		};
	});

test.beforeEach(async ({ page }) => {
	await page.setViewportSize({ width: 1920, height: 1080 });
});

test('a persisted tiles choice loads without crashing or scrolling the page', async ({ page }) => {
	test.setTimeout(120_000);
	const errors = watch(page);
	const seen = new Set<string>();
	await stubApi(page, seen);
	await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'tiles'));

	await page.goto('/sessions', { waitUntil: 'domcontentloaded' });
	await page.waitForSelector('[data-journey="session-tiles"] .tile', { timeout: 20_000 });
	await page.waitForTimeout(3000);

	const m = await metrics(page);
	const historyFetches = [...seen].filter((p) => p.includes('/conversation')).length;
	console.log(`${JSON.stringify(m)} historyFetches=${historyFetches}`);
	await page.screenshot({
		path: `/home/dorsk/.claude/artifacts/cctui-wave-023/tiles-crash/tiles-${process.env.TILES_TAG ?? 'after'}.png`
	});

	expect(errors.join('\n')).not.toContain('effect_update_depth_exceeded');
	expect(errors, 'no uncaught error on load').toEqual([]);
	// The window height IS the layout: the tiles area never grows the document.
	expect(m.docScrollH).toBe(m.winH);
	expect(m.mainH).toBe(m.winH);

	// Bounded load: never more panes than the readable-size cap, and one history
	// fetch per mounted pane rather than one per live session.
	const cap = Math.max(1, Math.floor(m.gridW / 360) * Math.floor(m.gridH / 220));
	expect(m.panes).toBe(Math.min(N, cap));
	expect(historyFetches).toBeLessThanOrEqual(m.panes);
	if (N > cap) await expect(page.getByText(`+${N - cap} more`)).toBeVisible();
});

test('?view=list rescues a browser stuck in tiles', async ({ page }) => {
	const errors = watch(page);
	await stubApi(page, new Set());
	await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'tiles'));

	await page.goto('/sessions?view=list', { waitUntil: 'domcontentloaded' });
	await page.waitForTimeout(3000);

	const m = await metrics(page);
	expect(m.panes).toBe(0);
	expect(m.view).toBe('list');
	expect(errors, 'no uncaught error').toEqual([]);
	// The hint is consumed, not left in the address bar.
	expect(new URL(page.url()).searchParams.get('view')).toBeNull();
});

test('a tiles render that took the tab down last time starts in the list', async ({ page }) => {
	await stubApi(page, new Set());
	await page.addInitScript(() => {
		localStorage.setItem('cctui_list_view', 'tiles');
		sessionStorage.setItem('cctui_tiles_booting', '1');
	});

	await page.goto('/sessions', { waitUntil: 'domcontentloaded' });
	await page.waitForTimeout(3000);

	const m = await metrics(page);
	expect(m.panes).toBe(0);
	expect(await page.evaluate(() => sessionStorage.getItem('cctui_tiles_booting'))).toBeNull();
});
