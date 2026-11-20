import { expect, test, type Page } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';

const SOURCE = {
	...session(0),
	name: 'big context',
	bucket: 'done',
	context_tokens: 160_000,
	context_window: 200_000,
	auto_followup_at: 150_000 as number | null,
	auto_followup_override: null as unknown
};
const SETTINGS = { autoFollowup: { enabled: true, threshold: { unit: 'tokens', value: 150_000 } } };

interface Stub {
	frames: Record<string, unknown>[];
	overrides: unknown[];
}

async function stubAuto(page: Page): Promise<Stub> {
	const row = { ...SOURCE };
	await stubApi(page, [row], { settings: SETTINGS });
	const stub: Stub = { frames: [], overrides: [] };
	await page.routeWebSocket('**/api/v1/ws', (ws) => {
		ws.onMessage((raw) => {
			const frame = JSON.parse(String(raw)) as Record<string, unknown>;
			stub.frames.push(frame);
			if (frame.type === 'message' && frame.client_msg_id) {
				ws.send(
					JSON.stringify({
						type: 'message_ack',
						session_id: frame.session_id,
						client_msg_id: frame.client_msg_id,
						ok: true
					})
				);
			}
		});
	});
	await page.route(
		(url) => url.pathname === `/api/v1/sessions/${SOURCE.id}/auto-followup`,
		(route) => {
			const body = route.request().postDataJSON() as { enabled: boolean | null };
			stub.overrides.push(body);
			row.auto_followup_override = body;
			row.auto_followup_at = body.enabled === false ? null : 150_000;
			return route.fulfill({ json: row });
		}
	);
	return stub;
}

async function open(page: Page) {
	await page.goto('/sessions');
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
}

const composer = (page: Page) => page.locator('[data-journey="composer"]');
const send = (page: Page) => composer(page).locator('[data-tsu="SplitButton"] .split-main button');
const message = (page: Page) => composer(page).locator('textarea').first();
const meter = (page: Page) => page.locator('[data-journey="head-meta"]').getByTestId('context-meter');
const sent = (stub: Stub) => stub.frames.filter((f) => f.type === 'message');

test.describe('auto follow-up', () => {
	test.use({ viewport: { width: 1536, height: 900 } });

	test('past the threshold the composer and the meter say so', async ({ page }) => {
		await stubAuto(page);
		await open(page);
		await expect(send(page)).toHaveText('Follow-up · 160k');
		await expect(meter(page)).toContainText('160k / 200k · 80%');
		await expect(meter(page).getByTestId('context-meter-mark')).toHaveCount(1);
		await expect(meter(page)).toHaveAttribute('title', /150k/);
	});

	test('Send here posts followup never', async ({ page }) => {
		const stub = await stubAuto(page);
		await open(page);
		await message(page).fill('stay in this session');
		await composer(page).locator('[data-tsu="SplitButton"] .split-caret button').click();
		await page.getByRole('menuitem', { name: 'Send here', exact: true }).click();
		await expect.poll(() => sent(stub).length).toBe(1);
		expect(sent(stub)[0]).toMatchObject({ content: 'stay in this session', followup: 'never' });
	});

	test('a plain send carries followup auto', async ({ page }) => {
		const stub = await stubAuto(page);
		await open(page);
		await message(page).fill('carry on');
		await message(page).press('Enter');
		await expect.poll(() => sent(stub).length).toBe(1);
		expect(sent(stub)[0]).toMatchObject({ content: 'carry on', followup: 'auto' });
	});

	test('turning the session override off removes the label', async ({ page }) => {
		const stub = await stubAuto(page);
		await open(page);
		await expect(send(page)).toHaveText('Follow-up · 160k');
		await meter(page).click();
		const menu = page.getByTestId('context-meter-menu');
		await menu.getByText('Off', { exact: true }).click();
		await menu.getByRole('button', { name: 'Apply' }).click();
		await expect.poll(() => stub.overrides).toEqual([{ enabled: false, threshold: null }]);
		await expect(composer(page)).not.toContainText('Follow-up');
		await expect(composer(page).getByRole('button', { name: 'Send', exact: true })).toBeVisible();
		await expect(meter(page).getByTestId('context-meter-mark')).toHaveCount(0);
	});
});
