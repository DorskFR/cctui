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
	await stubApi(page, []);
	await page.unroute('**/ws**');
	await page.routeWebSocket('**/api/v1/ws', (ws) => ws.close());
	await page.route(
		(url) => url.pathname === '/api/v1/presets',
		(route) => route.fulfill({ json: PRESETS })
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
			const webhook = { id: 'h1', user_id: 'u1', created_at: '2026-11-19T00:00:00Z', ...body };
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
	await expect(preset.locator('option')).toHaveText(['Pick a preset', 'Issue triage']);
	await preset.selectOption('p1');
	await expect(editor.getByTestId('template-recognised')).toContainText('{{payload.issue.title}}');
	await editor.getByLabel('Calls per minute').fill('60');
	await page.locator('dialog[open]').getByRole('button', { name: 'Save' }).click();

	await expect.poll(() => calls.length).toBe(1);
	expect(calls[0]).toMatchObject({ method: 'POST', path: '/webhooks' });
	expect(calls[0].body).toEqual({
		preset_id: 'p1',
		name: 'Issue tracker',
		prompt_template: '{{event}} {{payload.action}}: {{payload.issue.title}}',
		rate_per_minute: 60,
		enabled: true
	});

	const secret = page.getByTestId('webhook-secret');
	await expect(secret.getByLabel('Secret', { exact: true })).toHaveValue('whsec_stubbed0123456789');
	await expect(secret.getByLabel('Delivery URL', { exact: true })).toHaveValue(/\/api\/v1\/hooks\/h1$/);
	await expect(secret).toContainText('X-Hub-Signature-256');
	await expect(secret).toContainText('will not be shown again');
	await page.getByRole('button', { name: 'I have copied it' }).click();
	await expect(page.locator('dialog[open]')).toHaveCount(0);

	const row = page.getByTestId('webhook-row');
	await expect(row).toContainText('Issue tracker');
	await expect(row).toContainText('Issue triage');
	await expect(row.getByTestId('webhook-url')).toContainText('/api/v1/hooks/h1');
	await expect(page.getByText('whsec_stubbed0123456789')).toHaveCount(0);

	await row.getByText('Recent deliveries').click();
	const log = row.getByTestId('webhook-deliveries').locator('li');
	await expect(log).toHaveCount(2);
	await expect(log.nth(0)).toContainText('Dispatched');
	await expect(log.nth(0).getByRole('link')).toHaveAttribute('href', '/sessions/abcdef0123456789');
	await expect(log.nth(1)).toContainText('Rejected');
	await expect(log.nth(1)).toContainText('signature mismatch');
});
