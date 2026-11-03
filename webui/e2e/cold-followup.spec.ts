import { expect, test, type Page, type Request } from '@playwright/test';
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
const SPAWNED = { ...session(1), id: '00000000-0000-4000-8000-0000000000f1', name: 'follow-up', bucket: 'working' };
const INSTRUCTION = 'Carry on with the migration tests.';

interface Stub {
	spawns: Request[];
	archived: string[];
}

async function stubCold(page: Page, opts: { spawnFails?: boolean } = {}): Promise<Stub> {
	const sessions: unknown[] = [SOURCE];
	await stubApi(page, sessions);
	const stub: Stub = { spawns: [], archived: [] };
	await page.route(
		(url) => /^\/api\/v1\/sessions\/[^/]+\/brief$/.test(url.pathname),
		(route) =>
			route.fulfill({ json: { markdown: '**User:**\n\nq\n\n**Assistant:**\n\na', turns: 1, omitted: 0, truncated: false } })
	);
	await page.route(
		(url) => url.pathname === '/api/v1/sessions/spawn',
		(route) => {
			stub.spawns.push(route.request());
			if (opts.spawnFails) return route.fulfill({ status: 500, json: { error: 'no daemon on m1' } });
			sessions.push(SPAWNED);
			return route.fulfill({ json: { command_id: 'cmd-followup-1', status: 'dispatched', session_id: SPAWNED.id } });
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

function spawnBody(req: Request): Record<string, unknown> {
	const raw = req.postDataBuffer()?.toString('utf8') ?? '';
	const part = raw.match(/name="request"\r\n\r\n([\s\S]*?)\r\n--/);
	expect(part, 'the spawn carries its JSON request part').not.toBeNull();
	return JSON.parse(part![1]) as Record<string, unknown>;
}

async function openSource(page: Page) {
	await page.goto('/sessions');
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
}

const send = (page: Page) => page.locator('[data-journey="composer"] [data-tsu="SplitButton"] .split-main button');
const message = (page: Page) => page.locator('[data-journey="composer"] textarea').first();

async function expectFollowupLaunched(page: Page, stub: Stub) {
	await expect(page).toHaveURL(new RegExp(`/sessions/${SPAWNED.id}`), { timeout: 20_000 });
	expect(stub.spawns).toHaveLength(1);
	expect(spawnBody(stub.spawns[0])).toMatchObject({
		machine_id: SOURCE.machine_id,
		working_dir: SOURCE.working_dir,
		model: SOURCE.model,
		account: SOURCE.account_name,
		relation: 'followup',
		parent_session_id: SOURCE.id
	});
	expect(String(spawnBody(stub.spawns[0]).prompt).endsWith(INSTRUCTION)).toBe(true);
	await expect.poll(() => stub.archived).toEqual([SOURCE.id]);
	await expect(page.locator('dialog[open]')).toHaveCount(0);
}

test.describe('desktop', () => {
	test.use({ viewport: { width: 1536, height: 900 } });

	test('Enter on a cold session launches the follow-up, archives the source and opens the new session', async ({
		page
	}) => {
		test.setTimeout(60_000);
		const stub = await stubCold(page);
		await openSource(page);
		await expect(send(page)).toContainText('Follow-up ❄️ ~93');
		await expect(page.getByTestId('cold-offer')).toContainText('Send here instead');
		await message(page).fill(INSTRUCTION);
		await message(page).press('Enter');
		await expect(page.locator('dialog[open]')).toHaveCount(0);
		await expectFollowupLaunched(page, stub);
	});

	for (const chord of ['Meta+Enter', 'Control+Enter']) {
		test(`${chord} on a cold session launches the follow-up like Enter`, async ({ page }) => {
			test.setTimeout(60_000);
			const stub = await stubCold(page);
			await openSource(page);
			await expect(send(page)).toContainText('Follow-up');
			await message(page).fill(INSTRUCTION);
			await message(page).press(chord);
			await expectFollowupLaunched(page, stub);
		});
	}

	for (const chord of ['Enter', 'Meta+Enter']) {
		test(`${chord} in a cold tile launches the follow-up`, async ({ page }) => {
			test.setTimeout(60_000);
			const stub = await stubCold(page);
			await page.addInitScript(() => localStorage.setItem('cctui_list_view', 'tiles'));
			await page.goto('/sessions');
			const tile = page.locator('[data-journey="session-tiles"] .tile').first();
			await tile.locator('[data-journey="composer"] textarea').first().fill(INSTRUCTION);
			await expect(tile.locator('[data-journey="composer"] [data-tsu="SplitButton"]')).toContainText('Follow-up');
			await tile.locator('[data-journey="composer"] textarea').first().press(chord);
			await expectFollowupLaunched(page, stub);
		});
	}

	test('the Send menu offers Send here and the options modal above the schedule items', async ({ page }) => {
		await stubCold(page);
		await openSource(page);
		await message(page).fill(INSTRUCTION);
		await page.locator('[data-journey="composer"] [data-tsu="SplitButton"] .split-caret button').click();
		const items = page.getByRole('menuitem');
		await expect(items.nth(0)).toContainText('Send here (rebuild cache, ~93');
		await expect(items.nth(1)).toContainText('Follow-up with options');
		await items.nth(1).click();
		await expect(page.locator('dialog[open]').getByLabel('Archive the source session after launch')).toBeChecked();
	});

	test('a failed spawn archives nothing and keeps the text in the composer', async ({ page }) => {
		const stub = await stubCold(page, { spawnFails: true });
		await openSource(page);
		await message(page).fill(INSTRUCTION);
		await send(page).click();
		await expect.poll(() => stub.spawns.length).toBe(1);
		await expect(page.getByText(/no daemon on m1/)).toBeVisible();
		await expect(message(page)).toHaveValue(INSTRUCTION);
		expect(stub.archived).toEqual([]);
		expect(page.url()).not.toContain(SPAWNED.id);
	});
});

test.describe('mobile', () => {
	test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });

	test('tapping Send on a cold session launches the follow-up with no modal', async ({ page }) => {
		test.setTimeout(60_000);
		const stub = await stubCold(page);
		await openSource(page);
		await expect(send(page)).toContainText('Follow-up');
		await message(page).fill(INSTRUCTION);
		await send(page).tap();
		await expectFollowupLaunched(page, stub);
	});
});
