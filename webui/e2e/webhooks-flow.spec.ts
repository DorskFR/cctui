import { expect, test, type Page } from '@playwright/test';
import { stubApi } from './tiles.fixture';

type Body = Record<string, unknown>;
type Call = { method: string; path: string; body: Body | null };

const PRESETS = [
	{ id: 'p1', name: 'Issue triage', external_sources: true },
	{ id: 'p2', name: 'Nightly report', external_sources: false }
].map((p) => ({
	user_id: 'u1',
	prompt: 'Handle it',
	machine_id: 'm1',
	working_dir: '/home/alice/widgets',
	adapter_id: 'claude-code',
	model: null,
	permission_mode: 'auto',
	account_id: null,
	pool_id: null,
	no_account: false,
	dispatcher_id: null,
	profile: null,
	fresh_secs: 900,
	on_stale: 'new',
	max_batch: 1,
	cron: null,
	timezone: 'UTC',
	enabled: true,
	overlap: 'skip',
	next_fire_at: null,
	last_fire_at: null,
	then_preset_id: null,
	notify_url: null,
	created_at: '2026-11-19T00:00:00Z',
	updated_at: '2026-11-19T00:00:00Z',
	...p
}));

const DELIVERIES = [
	{
		id: 3,
		delivery_id: 'gh-333',
		outcome: 'over_budget',
		detail: 'over budget: its runs cost $5.20 of the $5.00 allowed per 24 hours',
		event_id: null,
		session_id: null,
		received_at: '2026-11-19T09:10:00Z'
	},
	{
		id: 2,
		delivery_id: 'gh-222',
		outcome: 'dispatched',
		detail: null,
		event_id: 'e2',
		session_id: 'abcdef0123456789',
		received_at: '2026-11-19T09:05:00Z'
	},
	{
		id: 1,
		delivery_id: 'gh-111',
		outcome: 'rejected',
		detail: 'signature mismatch',
		event_id: null,
		session_id: null,
		received_at: '2026-11-19T09:00:00Z'
	}
];

async function stub(page: Page) {
	const calls: Call[] = [];
	let hooks: Body[] = [];
	const presets: Body[] = PRESETS.map((p) => ({ ...p, env_keys: [] }));
	await stubApi(page, []);
	await page.unroute('**/ws**');
	await page.routeWebSocket('**/api/v1/ws', (ws) => ws.close());
	await page.route(
		(url) => url.pathname === '/api/v1/presets',
		(route) => route.fulfill({ json: presets })
	);
	await page.route(
		(url) => url.pathname.startsWith('/api/v1/presets/'),
		(route) => {
			const req = route.request();
			const path = new URL(req.url()).pathname.replace('/api/v1', '');
			const at = presets.findIndex((p) => path === `/presets/${p.id}`);
			if (at < 0) return route.fulfill({ status: 404, json: { error: 'no preset' } });
			if (req.method() === 'GET') return route.fulfill({ json: presets[at] });
			const body = (req.postDataJSON() ?? null) as Body | null;
			calls.push({ method: req.method(), path, body });
			presets[at] = { ...presets[at], ...body };
			return route.fulfill({ json: presets[at] });
		}
	);
	await page.route(
		(url) => url.pathname.startsWith('/api/v1/webhooks'),
		(route) => {
			const req = route.request();
			const path = new URL(req.url()).pathname.replace('/api/v1', '');
			const method = req.method();
			if (path.endsWith('/deliveries')) return route.fulfill({ json: DELIVERIES });
			if (method === 'GET') return route.fulfill({ json: hooks });
			const body = (req.postDataJSON() ?? null) as Body | null;
			calls.push({ method, path, body });
			const webhook = {
				id: 'h1',
				user_id: 'u1',
				created_at: '2026-11-19T00:00:00Z',
				url: 'https://hooks.example/hooks/h1',
				...body
			};
			hooks = [webhook];
			return route.fulfill({ status: 201, json: { webhook, secret: 'whsec_stubbed0123456789' } });
		}
	);
	return calls;
}

test('a webhook is created, its secret shown once, and its deliveries listed', async ({ page }) => {
	const calls = await stub(page);
	await page.goto('/webhooks');
	await expect(page.getByText('No webhooks yet.')).toBeVisible();

	await page.getByRole('button', { name: 'New webhook' }).click();
	const editor = page.getByTestId('webhook-editor');
	await editor.getByLabel('Name').fill('Issue tracker');
	const preset = editor.getByLabel('Preset', { exact: true });
	await expect(preset.locator('option')).toHaveText([
		'Pick a preset',
		'Issue triage',
		'Nightly report (external sources off)'
	]);
	await preset.selectOption('p1');
	await expect(editor.getByTestId('webhook-external-off')).toHaveCount(0);
	await expect(editor.getByTestId('template-recognised')).toContainText('{{payload.issue.title}}');
	await editor.getByLabel('Calls per minute').fill('60');
	await editor.getByLabel('Spend cap (USD)').fill('5');
	await page.locator('dialog[open]').getByRole('button', { name: 'Save' }).click();

	await expect.poll(() => calls.length).toBe(1);
	expect(calls[0]).toMatchObject({ method: 'POST', path: '/webhooks' });
	expect(calls[0].body).toEqual({
		preset_id: 'p1',
		name: 'Issue tracker',
		prompt_template: '{{event}} {{payload.action}}: {{payload.issue.title}}',
		rate_per_minute: 60,
		max_usd: 5,
		budget_window_hours: 24,
		enabled: true
	});

	const secret = page.getByTestId('webhook-secret');
	await expect(secret.getByLabel('Secret', { exact: true })).toHaveValue('whsec_stubbed0123456789');
	await expect(secret.getByLabel('Delivery URL', { exact: true })).toHaveValue('https://hooks.example/hooks/h1');
	await expect(secret).toContainText('X-Hub-Signature-256');
	await expect(secret).toContainText('will not be shown again');
	await page.getByRole('button', { name: 'I have copied it' }).click();
	await expect(page.locator('dialog[open]')).toHaveCount(0);

	const row = page.getByTestId('webhook-row');
	await expect(row).toContainText('Issue tracker');
	await expect(row).toContainText('Issue triage');
	await expect(row.getByTestId('webhook-url')).toHaveText('https://hooks.example/hooks/h1');
	await expect(row.getByTestId('webhook-budget')).toContainText('cap $5.00 per 24 h');
	await expect(page.getByText('whsec_stubbed0123456789')).toHaveCount(0);

	await row.getByText('Recent deliveries').click();
	const log = row.getByTestId('webhook-deliveries').locator('li');
	await expect(log).toHaveCount(3);
	await expect(log.nth(0)).toContainText('Over budget');
	await expect(log.nth(0)).toContainText('$5.20 of the $5.00');
	await expect(log.nth(1)).toContainText('Dispatched');
	await expect(log.nth(1).getByRole('link')).toHaveAttribute('href', '/sessions/abcdef0123456789');
	await expect(log.nth(2)).toContainText('Rejected');
	await expect(log.nth(2)).toContainText('signature mismatch');
});

test('a preset closed to external sources is opened from the webhook form', async ({ page }) => {
	const calls = await stub(page);
	await page.goto('/webhooks');
	await page.getByRole('button', { name: 'New webhook' }).click();
	const editor = page.getByTestId('webhook-editor');
	await editor.getByLabel('Preset', { exact: true }).selectOption('p2');
	const offer = editor.getByTestId('webhook-external-off');
	await expect(offer).toContainText('does not accept external sources');
	await expect(offer).toContainText('pick Ask or Auto');
	await offer.getByRole('button', { name: 'Accept external sources' }).click();

	await expect.poll(() => calls.length).toBe(1);
	expect(calls[0]).toMatchObject({ method: 'PUT', path: '/presets/p2' });
	const sent: Body = { ...PRESETS[1], env_keys: [], external_sources: true };
	for (const key of ['id', 'user_id', 'next_fire_at', 'last_fire_at', 'created_at', 'updated_at'])
		delete sent[key];
	expect(calls[0].body).toEqual(sent);
	await expect(offer).toHaveCount(0);
	await expect(editor.getByLabel('Preset', { exact: true }).locator('option:checked')).toHaveText('Nightly report');
});
