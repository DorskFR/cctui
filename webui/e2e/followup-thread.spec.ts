import { expect, test, type Page, type Request, type WebSocketRoute } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';

const HOUR = 3_600_000;
const T0 = Date.now() - 6 * HOUR;
const at = (h: number) => new Date(T0 + h * HOUR).toISOString();

const followupOf = (instruction: string) => ({
	relation: 'followup',
	followup_reason: 'cold',
	followup_instruction: instruction,
	name_source: 'user'
});

const A = { ...session(10), name: 'line A', status: 'archived', registered_at: at(0) };
const TURNS = 5;
const B = {
	...session(11),
	name: 'line B',
	status: 'archived',
	parent_id: A.id,
	registered_at: at(1),
	metadata: followupOf('Carry on in B.')
};
const C = {
	...session(12),
	name: 'line C',
	parent_id: B.id,
	bucket: 'done',
	registered_at: at(2),
	last_activity_at: new Date(Date.now() - 2 * HOUR).toISOString(),
	estimated_burst_tokens: 93_000,
	metadata: followupOf('Carry on in C.')
};
const D = { ...session(13), name: 'line D', parent_id: C.id, registered_at: at(5), metadata: followupOf('Carry on in D.') };
const rows = new Map<string, object>([
	[A.id, { ...A, next_session_id: B.id }],
	[B.id, { ...B, next_session_id: C.id }],
	[C.id, C],
	[D.id, D]
]);

const BRIEF =
	'<cctui-previous-sessions>\n<cctui-previous-conversation n="1" id="x">\nTHE WHOLE BRIEF\n</cctui-previous-conversation>\n</cctui-previous-sessions>';
const INSTRUCTION = 'Carry on in D.';

const text = (content: string, seq: number, hour: number) => ({
	type: 'text',
	content,
	meta: false,
	ts: T0 + hour * HOUR + seq * 1000,
	seq
});
const call = (tool: string, seq: number, hour: number) => ({
	type: 'tool_call',
	tool: 'Bash',
	input: { command: tool },
	ts: T0 + hour * HOUR + seq * 1000,
	seq
});

function turns(label: string, hour: number, from = 1) {
	const out: unknown[] = [];
	for (let i = 0; i < TURNS; i++) {
		out.push(text(`▷ User: ${label} question ${i}`, from + i * 3, hour));
		out.push(call(`${label}-tool-${i}`, from + i * 3 + 1, hour));
		out.push(text(`${label} answer ${i} ${'lorem ipsum '.repeat(30)}`, from + i * 3 + 2, hour));
	}
	return out;
}

const entry = (sessionId: string, generation: number, event: unknown, injected?: string) => ({
	entry: 'event',
	session_id: sessionId,
	generation,
	event,
	...(injected ? { injected } : {})
});
const marker = (generation: number, from: string, to: string, instruction: string, hour: number) => ({
	entry: 'followup_marker',
	generation,
	from,
	to,
	at: at(hour),
	reason: 'cold',
	instruction
});

const generations = [A, B, C].map((s, i) => ({
	session_id: s.id,
	generation: i + 1,
	readable: true,
	name: s.name,
	registered_at: s.registered_at
}));

const priorEntries = [
	...turns('alpha', 0).map((e) => entry(A.id, 1, e)),
	marker(2, A.id, B.id, 'Carry on in B.', 1),
	entry(B.id, 2, { ...text(BRIEF, 1, 1), meta: true }, 'previous_sessions'),
	entry(B.id, 2, text('▷ User: Carry on in B.', 1, 1)),
	...turns('beta', 1, 2).map((e) => entry(B.id, 2, e)),
	marker(3, B.id, C.id, 'Carry on in C.', 2)
];

const headEvents = [text(`▷ User: ${BRIEF}\n\nCarry on in C.`, 1, 2), ...turns('gamma', 2, 2)];

interface Stub {
	followups: Request[];
	socket: () => WebSocketRoute;
}

async function stubLine(page: Page): Promise<Stub> {
	const list: unknown[] = [{ ...C }];
	await stubApi(page, list);
	const stub: Stub = {
		followups: [],
		socket: () => {
			throw new Error('the page never opened its socket');
		}
	};
	await page.routeWebSocket('**/api/v1/ws', (ws) => {
		ws.onMessage(() => {});
		stub.socket = () => ws;
	});
	await page.route(
		(url) => /^\/api\/v1\/sessions\/[^/]+$/.test(url.pathname),
		(route) => {
			const id = new URL(route.request().url()).pathname.split('/')[4];
			const row = rows.get(id);
			return row ? route.fulfill({ json: row }) : route.fulfill({ status: 404, json: { error: 'not found' } });
		}
	);
	await page.route(
		(url) => /^\/api\/v1\/sessions\/[^/]+\/thread$/.test(url.pathname),
		(route) =>
			route.fulfill({
				json: {
					sessions: [A, B, C, ...(list.some((s) => (s as { id: string }).id === D.id) ? [D] : [])].map((s) => ({
						id: s.id,
						name: s.name,
						working_dir: s.working_dir,
						registered_at: s.registered_at,
						parent_id: s.parent_id
					}))
				}
			})
	);
	await page.route(
		(url) => /^\/api\/v1\/sessions\/[^/]+\/conversation$/.test(url.pathname),
		(route) => {
			const url = new URL(route.request().url());
			const id = url.pathname.split('/')[4];
			if (url.searchParams.get('thread') !== '1') {
				return route.fulfill({ json: id === C.id ? headEvents : [] });
			}
			const base = { thread_root_id: A.id, head: id, generations };
			if (url.searchParams.get('limit') === '1') return route.fulfill({ json: { ...base, entries: [] } });
			return route.fulfill({ json: { ...base, entries: priorEntries, next_cursor: null } });
		}
	);
	await page.route(
		(url) => /^\/api\/v1\/sessions\/[^/]+\/followup$/.test(url.pathname),
		(route) => {
			stub.followups.push(route.request());
			list.push(D);
			return route.fulfill({
				status: 202,
				json: { followup_id: 'f-1', from: C.id, to: null, state: 'pending', error: null }
			});
		}
	);
	return stub;
}

const drawer = (page: Page) => page.locator('[data-journey="conversation"]');
const markers = (page: Page) => page.locator('[data-journey="followup-marker"]');
const message = (page: Page) => page.locator('[data-journey="composer"] textarea').first();
const send = (page: Page) => page.locator('[data-journey="composer"] [data-tsu="SplitButton"] .split-main button');
const scroller = (page: Page) => drawer(page).locator('.conv');

function superseded(instruction = INSTRUCTION) {
	return JSON.stringify({
		type: 'session_superseded',
		followup_id: 'f-1',
		from: C.id,
		to: D.id,
		reason: 'cold',
		instruction,
		archived: true,
		archive_error: null,
		at: new Date().toISOString()
	});
}

async function openLine(page: Page, id = C.id) {
	await page.goto(`/sessions/${id}`);
	await drawer(page).waitFor();
	await expect(markers(page)).toHaveCount(2, { timeout: 20_000 });
}

async function tagPage(page: Page) {
	await drawer(page).evaluate((el) => (el.dataset.probe = 'kept'));
	await page.evaluate(() => ((window as unknown as { __kept: boolean }).__kept = true));
	return page.evaluate(() => history.length);
}
async function expectSamePage(page: Page, historyLength: number) {
	await expect(drawer(page)).toHaveAttribute('data-probe', 'kept');
	expect(await page.evaluate(() => (window as unknown as { __kept?: boolean }).__kept)).toBe(true);
	expect(await page.evaluate(() => history.length)).toBe(historyLength);
}

async function followUpFromComposer(page: Page, stub: Stub, submit: () => Promise<void>) {
	const historyLength = await tagPage(page);
	await message(page).fill(INSTRUCTION);
	await submit();
	await expect.poll(() => stub.followups.length).toBe(1);
	expect(stub.followups[0].postDataJSON()).toMatchObject({
		instruction: INSTRUCTION,
		reason: 'cold',
		archive_source: true,
		idempotency_key: expect.any(String)
	});
	await expect(markers(page)).toHaveCount(3);
	await expect(scroller(page).getByText(INSTRUCTION)).toBeVisible();
	stub.socket().send(superseded());
	await expect(page).toHaveURL(new RegExp(`/sessions/${D.id}`));
	await expect(markers(page).last()).toHaveAttribute('data-followup-to', D.id);
	stub.socket().send(
		JSON.stringify({ type: 'stream', session_id: D.id, data: text('the first reply from D', 1, 5) })
	);
	const reply = drawer(page).locator('.conv .bubble', { hasText: 'the first reply from D' });
	await expect(reply).toBeVisible();
	const markerTop = (await markers(page).last().boundingBox())?.y ?? Infinity;
	const instructionTop = (await scroller(page).getByText(INSTRUCTION).boundingBox())?.y ?? -1;
	const replyTop = (await reply.boundingBox())?.y ?? -1;
	expect(instructionTop).toBeGreaterThan(markerTop);
	expect(replyTop).toBeGreaterThan(instructionTop);
	await expectSamePage(page, historyLength);
}

test.describe('desktop', () => {
	test.use({ viewport: { width: 1536, height: 900 } });

	test('A→B→C opens as one scroll with two markers and no previous-conversation block', async ({ page }) => {
		test.setTimeout(60_000);
		await stubLine(page);
		await openLine(page);
		await expect(markers(page).nth(0)).toHaveAttribute('data-followup-to', B.id);
		await expect(markers(page).nth(1)).toHaveAttribute('data-followup-to', C.id);
		await expect(markers(page).nth(0)).toContainText('Continued in a new session');
		await scroller(page).evaluate((el) => (el.scrollTop = 0));
		for (const tool of [`alpha-tool-${TURNS - 1}`, 'beta-tool-0', `gamma-tool-${TURNS - 1}`]) {
			await expect(drawer(page).getByText(tool).first()).toBeAttached();
		}
		await expect(drawer(page).getByText('Previous conversation')).toHaveCount(0);
		await expect(drawer(page).getByText('THE WHOLE BRIEF')).toHaveCount(0);
		await expect(drawer(page).getByText('Carry on in B.')).toHaveCount(1);
		await expect(drawer(page).getByText('Carry on in C.')).toHaveCount(1);
	});

	test('a follow-up from the composer swaps the head in place, under a new marker', async ({ page }) => {
		test.setTimeout(60_000);
		const stub = await stubLine(page);
		await openLine(page);
		await expect(send(page)).toContainText('Follow-up');
		await followUpFromComposer(page, stub, () => message(page).press('Enter'));
		await expect(message(page)).toHaveValue('');
	});

	test('scroll position and an unsent draft survive the swap', async ({ page }) => {
		test.setTimeout(60_000);
		const stub = await stubLine(page);
		await openLine(page);
		const historyLength = await tagPage(page);
		await scroller(page).hover();
		await page.mouse.wheel(0, -300);
		await page.waitForTimeout(300);
		const before = await scroller(page).evaluate((el) => el.scrollTop);
		expect(before).toBeGreaterThan(0);
		await message(page).fill('an unsent draft');
		stub.socket().send(superseded('Carried on elsewhere.'));
		await expect(page).toHaveURL(new RegExp(`/sessions/${D.id}`));
		await expect(markers(page)).toHaveCount(3);
		await expect(message(page)).toHaveValue('an unsent draft');
		const after = await scroller(page).evaluate((el) => el.scrollTop);
		expect(Math.abs(after - before)).toBeLessThan(40);
		await expectSamePage(page, historyLength);
	});

	test("opening B's URL lands at B's marker and shows the head", async ({ page }) => {
		test.setTimeout(60_000);
		await stubLine(page);
		await page.goto(`/sessions/${B.id}`);
		await drawer(page).waitFor();
		await expect(markers(page)).toHaveCount(2, { timeout: 20_000 });
		const markerB = drawer(page).locator(`[data-followup-to="${B.id}"]`);
		await expect(markerB).toBeInViewport();
		await expect(drawer(page).locator('[data-journey="header"]')).toContainText(C.name);
		await expect(message(page)).toBeEditable();
	});
});

test.describe('mobile', () => {
	test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });

	test('tapping Send on the cold head swaps it in place', async ({ page }) => {
		test.setTimeout(60_000);
		const stub = await stubLine(page);
		await openLine(page);
		await followUpFromComposer(page, stub, () => send(page).tap());
	});
});
