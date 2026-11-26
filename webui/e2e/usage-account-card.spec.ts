import { expect, test, type Page } from '@playwright/test';
import { session, stubApi, watch } from './tiles.fixture';

const SHOTS = process.env.ACCOUNT_CARD_SHOTS;
const ACCT = 'acct-1';
const resets = (h: number) => new Date(Date.now() + h * 3_600_000).toISOString();

const provider = (id: string, kind: string, soft_limits: Record<string, unknown> | null = null) => ({
	id,
	account_id: ACCT,
	provider: kind,
	family: kind,
	models: null,
	model_aliases: null,
	managed: false,
	base_url: null,
	auth_scheme: 'oauth',
	provider_account_id: null,
	expires_at: null,
	created_at: '2026-01-01T00:00:00Z',
	last_used_at: null,
	request_count: 0,
	bytes_transferred: 0,
	total_tokens: 0,
	est_cost_usd: 0,
	soft_limits,
	usage_notices: null,
	header_pin: false,
	needs_reauth: false,
	last_auth_error: null,
	last_auth_error_at: null,
	settings_json: null,
	provider_settings: null,
	rate_limits: null,
	usage_probe: null
});

const PROVIDERS = [
	provider('p-claude', 'anthropic'),
	provider('p-fw', 'fireworks', {
		session_usd: { cap_usd: 1 },
		usd_5h: { cap_usd: 10 },
		usd_7d: { cap_usd: 25 }
	}),
	provider('p-codex', 'openai', { weekly_all: { cap_pct: 95 } })
];

const ACCOUNT = {
	id: ACCT,
	name: 'alpha',
	emoji: null,
	user_id: 'u1',
	user_name: 'dorsk',
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z',
	providers: PROVIDERS,
	pool_eligible: false,
	pool_weight: 1,
	env_names: []
};

const pct = (key: string, label: string, utilization: number, h: number, pace?: unknown) => ({
	key,
	kind: 'percent',
	label,
	utilization,
	resets_at: resets(h),
	...(pace ? { pace } : {})
});
const usd = (key: string, label: string) => ({ key, kind: 'usd', label, utilization: 0, amount_usd: 0 });

const WINDOWS: Record<string, unknown[]> = {
	'p-claude': [pct('session', '5h', 25, 1), pct('weekly_all', '7d', 58, 17), pct('weekly_model:fable', 'fable', 46, 17)],
	'p-fw': [usd('session_usd', 'Session'), usd('usd_5h', '5h'), usd('usd_7d', '7d')],
	'p-codex': [
		pct('session', '5h', 44, 0.2),
		pct('weekly_all', '7d', 9, 160, { elapsed_fraction: 0.02, expected_pct: 2, ratio: 4.5 })
	]
};

const degraded = {
	family: 'anthropic',
	indicator: 'minor',
	description: 'Elevated errors',
	url: 'https://status.example.test',
	components: [{ name: 'API', status: 'degraded_performance' }],
	incidents: []
};

const usageFor = (p: (typeof PROVIDERS)[number]) => ({
	account_id: p.id,
	provider: p.provider,
	usage: null,
	windows: WINDOWS[p.id],
	age_secs: 0,
	limit_reset: null,
	limit_resets: [],
	provider_status: p.provider === 'fireworks' ? undefined : { ...degraded, family: p.family }
});

async function stub(page: Page, dockWidth?: number) {
	await stubApi(page, [session(0)], {
		settings: dockWidth ? { statsDock: { enabled: true, side: 'left', width: dockWidth } } : {}
	});
	await page.route('**/api/v1/accounts**', (route) => {
		const path = new URL(route.request().url()).pathname.replace('/api/v1', '');
		if (path === '/accounts') return route.fulfill({ json: [ACCOUNT] });
		if (path === '/accounts/usage')
			return route.fulfill({
				json: PROVIDERS.map((p) => ({
					...usageFor(p),
					account: ACCT,
					account_name: 'alpha',
					account_emoji: null,
					header_pin: false
				}))
			});
		if (path === '/accounts/usage/closes') return route.fulfill({ json: { closes: [], summary: [] } });
		const history = /^\/accounts\/([^/]+)\/usage\/history$/.exec(path);
		if (history) return route.fulfill({ json: { account_id: history[1], samples: [] } });
		const one = /^\/accounts\/([^/]+)\/usage$/.exec(path);
		if (one) {
			const p = PROVIDERS.find((x) => x.id === one[1]);
			if (p) return route.fulfill({ json: usageFor(p) });
		}
		return route.fallback();
	});
}

const layout = (page: Page, root: string) =>
	page.evaluate((root) => {
		const card = document.querySelector(`${root} article.acct`) as HTMLElement;
		const cells = [...card.querySelectorAll<HTMLElement>('.columns > *')].map((el) => {
			const r = el.getBoundingClientRect();
			const cs = getComputedStyle(el);
			const rule = (side: 'left' | 'right') =>
				cs.getPropertyValue(`border-${side}-style`) !== 'none' &&
				Number.parseFloat(cs.getPropertyValue(`border-${side}-width`)) > 0;
			return { left: r.left, right: r.right, top: r.top, height: r.height, ruleL: rule('left'), ruleR: rule('right') };
		});
		const codex = [...card.querySelectorAll<HTMLElement>('[data-tsu="CapBar"]')].at(-1)!;
		const track = codex.querySelector('.track') as HTMLElement;
		const t = track.getBoundingClientRect();
		const after = getComputedStyle(track, '::after');
		const handle = (track.querySelector('.handle') as HTMLElement).getBoundingClientRect();
		const nameRow = card.querySelector('header.head') as HTMLElement;
		return {
			cells,
			track: { left: t.left, right: t.right, top: t.top, bottom: t.bottom },
			handle: { left: handle.left, right: handle.right, top: handle.top, bottom: handle.bottom },
			zone: {
				display: after.display,
				edge: after.boxShadow,
				background: after.backgroundColor,
				trackBackground: getComputedStyle(track).backgroundColor
			},
			head: nameRow.innerText
		};
	}, root);

type Layout = Awaited<ReturnType<typeof layout>>;

function assertSeparators(l: Layout, tag: string, expectRows: 'side' | 'stacked') {
	const same = (a: number, b: number) => Math.abs(a - b) < 1.5;
	const rows = new Set(l.cells.map((c) => Math.round(c.top))).size;
	if (expectRows === 'stacked') expect(rows, `${tag}: provider sections stack`).toBe(l.cells.length);
	else expect(rows, `${tag}: provider sections sit side by side`).toBeLessThan(l.cells.length);
	for (const [i, c] of l.cells.entries()) {
		const row = l.cells.filter((o) => same(o.top, c.top));
		if (c.ruleR)
			expect.soft(row.some((o) => same(o.left, c.right)), `${tag}: column ${i} right rule has a neighbour`).toBe(true);
		if (c.ruleL)
			expect.soft(row.some((o) => same(o.right, c.left)), `${tag}: column ${i} left rule has a neighbour`).toBe(true);
		if (c.ruleL || c.ruleR)
			expect.soft(c.height, `${tag}: column ${i} rule spans the row`).toBeCloseTo(Math.max(...row.map((o) => o.height)), 0);
		const next = row.find((o) => same(o.left, c.right));
		if (next) expect.soft(c.ruleR || next.ruleL, `${tag}: column ${i} is divided from its neighbour`).toBe(true);
	}
}

function assertCapZone(l: Layout, tag: string) {
	expect.soft(l.handle.right, `${tag}: cap handle inside the track`).toBeLessThanOrEqual(l.track.right + 0.5);
	expect.soft(l.zone.edge, `${tag}: over-cap stretch has no outline of its own`).toMatch(/^none$|rgba\(0, 0, 0, 0\)|transparent/);
}

test.describe('account card', () => {
	test('stats dock at 460px', async ({ page }) => {
		const errors = watch(page);
		await page.setViewportSize({ width: 1400, height: 1100 });
		await stub(page, 460);
		await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'list'));
		await page.goto('/sessions', { waitUntil: 'domcontentloaded' });
		const card = page.locator('aside.dock article.acct');
		await expect(card).toBeVisible({ timeout: 30_000 });
		await expect(card.locator('[data-tsu="CapBar"]')).toHaveCount(8, { timeout: 20_000 });
		const l = await layout(page, 'aside.dock');
		console.log('dock', JSON.stringify(l));
		if (SHOTS) await card.screenshot({ path: `${SHOTS}/dock-460.png` });
		expect.soft(l.head).not.toMatch(/Claude|Codex/);
		assertSeparators(l, 'dock', 'stacked');
		assertCapZone(l, 'dock');
		expect(errors).toEqual([]);
	});

	test('accounts page at 1600px', async ({ page }) => {
		await page.setViewportSize({ width: 1600, height: 1000 });
		await stub(page);
		await page.goto('/settings/accounts', { waitUntil: 'domcontentloaded' });
		const card = page.locator('main article.acct').first();
		await expect(card).toBeVisible({ timeout: 30_000 });
		await expect(card.locator('[data-tsu="CapBar"]')).toHaveCount(8, { timeout: 20_000 });
		const l = await layout(page, 'main');
		console.log('page', JSON.stringify(l));
		if (SHOTS) await card.screenshot({ path: `${SHOTS}/page-1600.png` });
		expect.soft(l.head).not.toMatch(/Claude|Codex/);
		assertSeparators(l, 'page', 'side');
		assertCapZone(l, 'page');
	});
});
