import { expect, test, type Locator, type Page, type Request, type WebSocketRoute } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';

const HOUR = 3_600_000;
const SOURCE = {
	...session(0),
	name: 'cold source',
	bucket: 'done',
	account_name: 'acct-a',
	estimated_burst_tokens: 93_000,
	last_activity_at: new Date(Date.now() - 2 * HOUR).toISOString()
};
const SPAWNED = {
	...session(1),
	id: '00000000-0000-4000-8000-0000000000f1',
	name: 'follow-up',
	bucket: 'working',
	parent_id: SOURCE.id,
	metadata: { relation: 'followup' }
};
const INSTRUCTION = 'Carry on with the migration tests.';

interface Stub {
	followups: Request[];
	archived: string[];
	spawns: Request[];
	socket: () => WebSocketRoute;
}

async function stubCold(page: Page, opts: { refuse?: boolean } = {}): Promise<Stub> {
	const sessions: unknown[] = [SOURCE];
	await stubApi(page, sessions);
	const stub: Stub = {
		followups: [],
		archived: [],
		spawns: [],
		socket: () => {
			throw new Error('the page never opened its socket');
		}
	};
	await page.routeWebSocket('**/api/v1/ws', (ws) => {
		ws.onMessage(() => {});
		stub.socket = () => ws;
	});
	await page.route(
		(url) => url.pathname === `/api/v1/sessions/${SPAWNED.id}`,
		(route) => route.fulfill({ json: SPAWNED })
	);
	await page.route(
		(url) => /^\/api\/v1\/sessions\/[^/]+\/followup$/.test(url.pathname),
		(route) => {
			stub.followups.push(route.request());
			if (opts.refuse) return route.fulfill({ status: 502, json: { error: 'no daemon on m1' } });
			sessions.push(SPAWNED);
			return route.fulfill({
				status: 202,
				json: { followup_id: 'f-1', from: SOURCE.id, to: null, state: 'pending', error: null }
			});
		}
	);
	await page.route(
		(url) => url.pathname === '/api/v1/sessions/spawn',
		(route) => {
			stub.spawns.push(route.request());
			return route.fulfill({ status: 500, json: { error: 'the cold send must not spawn' } });
		}
	);
	await page.route(
		(url) => /^\/api\/v1\/sessions\/[^/]+\/archive$/.test(url.pathname),
		(route) => {
			stub.archived.push(new URL(route.request().url()).pathname.split('/')[4]);
			return route.fulfill({ status: 204, body: '' });
		}
	);
	return stub;
}

const superseded = JSON.stringify({
	type: 'session_superseded',
	followup_id: 'f-1',
	from: SOURCE.id,
	to: SPAWNED.id,
	reason: 'cold',
	instruction: INSTRUCTION,
	archived: true,
	archive_error: null,
	at: new Date().toISOString()
});

async function openSource(page: Page) {
	await page.goto('/sessions');
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
}

const drawer = (page: Page) => page.locator('[data-journey="conversation"]');
const send = (scope: Page | Locator) =>
	scope.locator('[data-journey="composer"] [data-tsu="SplitButton"] .split-main button');
const message = (scope: Page | Locator) => scope.locator('[data-journey="composer"] textarea').first();
const markers = (scope: Page | Locator) => scope.locator('[data-journey="followup-marker"]');

async function tag(page: Page) {
	await drawer(page).evaluate((el) => (el.dataset.probe = 'kept'));
	return page.evaluate(() => history.length);
}

async function expectPosted(page: Page, stub: Stub, scope: Page | Locator = page) {
	await expect.poll(() => stub.followups.length, { timeout: 20_000 }).toBe(1);
	expect(new URL(stub.followups[0].url()).pathname).toBe(`/api/v1/sessions/${SOURCE.id}/followup`);
	expect(stub.followups[0].postDataJSON()).toEqual({
		instruction: INSTRUCTION,
		reason: 'cold',
		archive_source: true,
		idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/)
	});
	await expect(markers(scope)).toHaveCount(1);
	await expect(page.locator('dialog[open]')).toHaveCount(0);
	expect(stub.spawns).toEqual([]);
	expect(stub.archived).toEqual([]);
}

async function expectSwappedInPlace(page: Page, stub: Stub, historyLength: number) {
	await expectPosted(page, stub);
	await expect(message(page)).toHaveValue('');
	stub.socket().send(superseded);
	await expect(page).toHaveURL(new RegExp(`/sessions/${SPAWNED.id}`), { timeout: 20_000 });
	await expect(markers(page)).toHaveCount(1);
	await expect(markers(page).first()).toHaveAttribute('data-followup-to', SPAWNED.id);
	await expect(drawer(page).getByText(INSTRUCTION)).toBeVisible();
	await expect(drawer(page)).toHaveAttribute('data-probe', 'kept');
	expect(await page.evaluate(() => history.length)).toBe(historyLength);
	expect(stub.archived).toEqual([]);
}

test.describe('desktop', () => {
	test.use({ viewport: { width: 1536, height: 900 } });

	test('Enter on a cold session posts the follow-up and swaps to it in place', async ({ page }) => {
		test.setTimeout(60_000);
		const stub = await stubCold(page);
		await openSource(page);
		await expect(send(page)).toContainText('Follow-up ❄️ ~93');
		await expect(page.getByTestId('cold-offer')).toContainText('Send here instead');
		const historyLength = await tag(page);
		await message(page).fill(INSTRUCTION);
		await message(page).press('Enter');
		await expectSwappedInPlace(page, stub, historyLength);
	});

	for (const chord of ['Meta+Enter', 'Control+Enter']) {
		test(`${chord} on a cold session launches the follow-up like Enter`, async ({ page }) => {
			test.setTimeout(60_000);
			const stub = await stubCold(page);
			await openSource(page);
			await expect(send(page)).toContainText('Follow-up');
			const historyLength = await tag(page);
			await message(page).fill(INSTRUCTION);
			await message(page).press(chord);
			await expectSwappedInPlace(page, stub, historyLength);
		});
	}

	test('Shift+Enter on a cold session only inserts a newline', async ({ page }) => {
		const stub = await stubCold(page);
		await openSource(page);
		await expect(send(page)).toContainText('Follow-up');
		await message(page).fill(INSTRUCTION);
		await message(page).press('Shift+Enter');
		await message(page).pressSequentially('more');
		await expect(message(page)).toHaveValue(`${INSTRUCTION}\nmore`);
		expect(stub.followups).toHaveLength(0);
	});

	for (const chord of ['Enter', 'Meta+Enter']) {
		test(`${chord} in a cold tile posts the follow-up and swaps the tile in place`, async ({ page }) => {
			test.setTimeout(60_000);
			const stub = await stubCold(page);
			await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'tiles'));
			await page.goto('/sessions');
			const tile = page.locator('[data-journey="session-tiles"] .tile').first();
			await message(tile).fill(INSTRUCTION);
			await expect(tile.locator('[data-journey="composer"] [data-tsu="SplitButton"]')).toContainText('Follow-up');
			await message(tile).press(chord);
			await expectPosted(page, stub, tile);
			stub.socket().send(superseded);
			await expect(markers(tile).first()).toHaveAttribute('data-followup-to', SPAWNED.id);
			await expect(page).not.toHaveURL(new RegExp(SPAWNED.id));
		});
	}

	test('the Send menu offers Send here and the options modal above the schedule items', async ({ page }) => {
		const stub = await stubCold(page);
		await openSource(page);
		await message(page).fill(INSTRUCTION);
		await page.locator('[data-journey="composer"] [data-tsu="SplitButton"] .split-caret button').click();
		const items = page.getByRole('menuitem');
		await expect(items.nth(0)).toContainText('Send here (rebuild cache, ~93');
		await expect(items.nth(1)).toContainText('Follow-up with options');
		await items.nth(1).click();
		await expect(page.locator('dialog[open]').getByLabel('Archive the source session after launch')).toBeChecked();
		// The follow-up form replaces the conversation; closing it must not close the form.
		await expect(drawer(page)).toHaveCount(0);
		await expect(page.locator('dialog[open]')).toHaveCount(1);
		expect(stub.followups).toHaveLength(0);
	});

	test('a refused follow-up archives nothing and keeps the text in the composer', async ({ page }) => {
		const stub = await stubCold(page, { refuse: true });
		await openSource(page);
		await message(page).fill(INSTRUCTION);
		await send(page).click();
		await expect.poll(() => stub.followups.length).toBe(1);
		await expect(page.getByText(/no daemon on m1/)).toBeVisible();
		await expect(message(page)).toHaveValue(INSTRUCTION);
		await expect(markers(page)).toHaveCount(0);
		expect(stub.archived).toEqual([]);
		expect(page.url()).not.toContain(SPAWNED.id);
	});

	test('a follow-up that fails after launch restores the instruction and stays put', async ({ page }) => {
		const stub = await stubCold(page);
		await openSource(page);
		await message(page).fill(INSTRUCTION);
		await message(page).press('Enter');
		await expectPosted(page, stub);
		await expect(message(page)).toHaveValue('');
		stub.socket().send(
			JSON.stringify({
				type: 'followup_failed',
				followup_id: 'f-1',
				from: SOURCE.id,
				reason: 'cold',
				instruction: INSTRUCTION,
				error: 'worker never registered'
			})
		);
		await expect(page.getByText(/worker never registered/)).toBeVisible();
		await expect(message(page)).toHaveValue(INSTRUCTION);
		await expect(markers(page)).toHaveCount(0);
		expect(page.url()).not.toContain(SPAWNED.id);
	});
});

test.describe('mobile', () => {
	test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });

	test('tapping Send on a cold session posts the follow-up and swaps in place, no modal', async ({ page }) => {
		test.setTimeout(60_000);
		const stub = await stubCold(page);
		await openSource(page);
		await expect(send(page)).toContainText('Follow-up');
		const historyLength = await tag(page);
		await message(page).fill(INSTRUCTION);
		await send(page).tap();
		await expectSwappedInPlace(page, stub, historyLength);
	});
});
