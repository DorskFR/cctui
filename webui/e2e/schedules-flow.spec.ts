import { expect, test, type Page } from '@playwright/test';
import { stubApi } from './tiles.fixture';

type Body = Record<string, unknown>;
type Call = { method: string; path: string; body: Body | null };

const MACHINE = {
	machine_id: 'm1',
	name: 'workbench',
	display_name: 'workbench',
	hue: null,
	liveness: 'online',
	connected: true,
	last_seen_at: '2026-11-19T00:00:00Z',
	resources: null,
	updated_at: null
};

async function stub(page: Page) {
	const calls: Call[] = [];
	let presets: Body[] = [];
	await stubApi(page, []);
	await page.unroute('**/ws**');
	await page.routeWebSocket('**/api/v1/ws', (ws) => ws.close());
	await page.route(
		(url) => url.pathname === '/api/v1/machines/resources',
		(route) => route.fulfill({ json: [MACHINE] })
	);
	await page.route(
		(url) => url.pathname === '/api/v1/machines/m1/adapters',
		(route) =>
			route.fulfill({
				json: [{ adapter_id: 'claude-code', enabled: true, config: {}, pinned: false, default_enabled: true }]
			})
	);
	await page.route(
		(url) => url.pathname.startsWith('/api/v1/presets'),
		(route) => {
			const req = route.request();
			const path = new URL(req.url()).pathname.replace('/api/v1', '');
			const method = req.method();
			if (method === 'GET') return route.fulfill({ json: presets });
			const body = (req.postDataJSON() ?? null) as Body | null;
			calls.push({ method, path, body });
			if (path.endsWith('/run')) {
				return route.fulfill({ status: 202, json: { event_id: 'e1', lane_id: 'l1' } });
			}
			const saved = {
				id: 'p1',
				user_id: 'u1',
				created_at: '2026-11-19T00:00:00Z',
				updated_at: '2026-11-19T00:00:00Z',
				last_fire_at: null,
				...body,
				next_fire_at: body?.enabled === false ? null : '2026-11-19T22:00:00Z'
			};
			presets = [saved];
			return route.fulfill({ status: method === 'POST' ? 201 : 200, json: saved });
		}
	);
	return calls;
}

test('a weekday schedule is created, paused and run by hand', async ({ page }) => {
	const calls = await stub(page);
	await page.goto('/schedules');
	await expect(page.getByText('No schedules yet.')).toBeVisible();

	await page.getByRole('button', { name: 'New schedule' }).click();
	const dialog = page.locator('dialog[open]').first();
	await dialog.getByLabel('Name').fill('Standup notes');
	await dialog.getByLabel('Prompt').fill('Prepare the standup notes');
	await dialog.getByRole('radio', { name: 'Weekdays' }).click();
	await dialog.getByLabel('Time of day').fill('07:00');
	await dialog.getByLabel('Timezone').selectOption('Asia/Tokyo');
	await expect(dialog.getByTestId('schedule-words')).toContainText('Weekdays at 07:00');
	await dialog.getByRole('button', { name: 'Save' }).click();

	await expect.poll(() => calls.length).toBe(1);
	expect(calls[0]).toMatchObject({ method: 'POST', path: '/presets' });
	expect(calls[0].body).toMatchObject({
		name: 'Standup notes',
		prompt: 'Prepare the standup notes',
		machine_id: 'm1',
		working_dir: '~',
		adapter_id: 'claude-code',
		cron: '0 7 * * 1-5',
		timezone: 'Asia/Tokyo',
		enabled: true,
		overlap: 'skip'
	});

	const row = page.getByTestId('schedule-row');
	await expect(row).toContainText('Standup notes');
	await expect(row).toContainText('Weekdays at 07:00');
	await expect(row).toContainText('Asia/Tokyo');
	await expect(page.locator('dialog[open]')).toHaveCount(0);

	await row.getByRole('switch', { name: 'Standup notes enabled' }).click();
	await expect.poll(() => calls.length).toBe(2);
	expect(calls[1]).toMatchObject({ method: 'PUT', path: '/presets/p1' });
	expect(calls[1].body).toMatchObject({ enabled: false, cron: '0 7 * * 1-5', timezone: 'Asia/Tokyo' });
	await expect(row).toContainText('Paused');

	await row.getByRole('button', { name: 'Run now' }).click();
	await expect.poll(() => calls.length).toBe(3);
	expect(calls[2]).toMatchObject({ method: 'POST', path: '/presets/p1/run' });
});
