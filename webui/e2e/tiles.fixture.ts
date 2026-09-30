import type { Page } from '@playwright/test';

export function session(i: number) {
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

const WIDE_CODE = Array.from(
	{ length: 40 },
	(_, i) =>
		`  const veryLongIdentifierNumber${i} = computeSomethingRatherInvolved(argumentOne, argumentTwo, argumentThree, ${i});`
).join('\n');

const WIDE_TABLE = [
	'| session | machine | working directory | adapter | model | status | last tool | tokens |',
	'| --- | --- | --- | --- | --- | --- | --- | --- |',
	...Array.from(
		{ length: 12 },
		(_, i) =>
			`| session ${i} | workbench-with-a-long-hostname | /home/dorsk/Documents/some/deeply/nested/project-${i} | claude_code | claude-opus-5 | working | Edit | 1234567 |`
	)
].join('\n');

const UNBREAKABLE =
	'/home/dorsk/Documents/a/really/long/path/that/never/breaks/because/it/has/no/spaces/at/all/and/keeps/going/' +
	'x'.repeat(400);

/** A transcript shaped like the ones that really overflow a pane: an unbreakable
 *  line, a code block wider than any tile, a wide table, and enough turns to
 *  overflow vertically. */
export function transcript(sessionId: string) {
	const base = Date.now() - 3_600_000;
	const events: unknown[] = [];
	let seq = 0;
	const at = () => base + seq * 1000;
	events.push({ type: 'text', content: `# ${sessionId}\n\n${UNBREAKABLE}`, meta: false, ts: at(), seq: seq++ });
	events.push({ type: 'text', content: '```ts\n' + WIDE_CODE + '\n```', meta: false, ts: at(), seq: seq++ });
	events.push({ type: 'text', content: WIDE_TABLE, meta: false, ts: at(), seq: seq++ });
	for (let i = 0; i < 40; i++) {
		events.push({
			type: 'text',
			content: `Turn ${i}: ${'lorem ipsum dolor sit amet consectetur adipiscing elit '.repeat(8)}`,
			meta: false,
			ts: at(),
			seq: seq++
		});
		events.push({
			type: 'tool_call',
			tool: 'Edit',
			input: { file_path: UNBREAKABLE },
			ts: at(),
			seq: seq++
		});
		events.push({
			type: 'tool_result',
			tool: 'Edit',
			output_summary: UNBREAKABLE,
			error: false,
			ts: at(),
			seq: seq++
		});
	}
	return events;
}

export interface StubOptions {
	settings?: Record<string, unknown>;
	/** Default is empty, so the crash spec keeps measuring layout not markdown. */
	conversation?: (sessionId: string) => unknown[];
	seen?: Set<string>;
}

// The whole API is stubbed: these specs exercise client-side layout, so the only
// thing that must be real is the app bundle.
export async function stubApi(page: Page, sessions: unknown[], opts: StubOptions = {}) {
	const seen = opts.seen ?? new Set<string>();
	await page.route('**/api/v1/**', async (route) => {
		const path = new URL(route.request().url()).pathname.replace('/api/v1', '');
		const json = (body: unknown) => route.fulfill({ json: body });
		if (path === '/me') return json({ id: 'u1', name: 'dorsk' });
		if (path === '/version') return json({ version: '0.23.0-beta.10' });
		if (path === '/settings') return json({ data: opts.settings ?? {} });
		if (path === '/labels') return json({ labels: [] });
		// DraftList, not the generic `[]`: serverDrafts iterates `.drafts`.
		if (path === '/drafts') return json({ drafts: [] });
		if (path.startsWith('/drafts/')) return json({});
		if (path.endsWith('/user-actions')) return json({ session_id: path.split('/')[2], items: [] });
		if (path === '/sessions' || path.startsWith('/sessions/search'))
			return json({ sessions, total: sessions.length });
		if (/^\/sessions\/[^/]+\/conversation/.test(path)) {
			seen.add(path);
			return json(opts.conversation?.(path.split('/')[2]) ?? []);
		}
		if (/^\/sessions\/[^/]+$/.test(path)) {
			const id = path.slice('/sessions/'.length);
			return json((sessions as { id: string }[]).find((s) => s.id === id) ?? sessions[0]);
		}
		return json([]);
	});
	// The socket is not what we are testing; let it fail closed.
	await page.route('**/ws**', (route) => route.abort());
	return seen;
}

export function watch(page: Page) {
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

export const metrics = (page: Page) =>
	page.evaluate(() => {
		const main = document.querySelector('main.content') as HTMLElement | null;
		const grid = document.querySelector('[data-journey="session-tiles"]') as HTMLElement | null;
		const tile = document.querySelector('[data-journey="session-tiles"] .tile') as HTMLElement | null;
		const conv = tile?.querySelector('.conv') as HTMLElement | null;
		return {
			mainH: main?.clientHeight ?? 0,
			gridW: grid?.clientWidth ?? 0,
			gridH: grid?.clientHeight ?? 0,
			tileW: tile?.clientWidth ?? 0,
			tileH: tile?.clientHeight ?? 0,
			convH: conv?.clientHeight ?? 0,
			panes: document.querySelectorAll('[data-journey="session-tiles"] .tile').length,
			docScrollH: document.documentElement.scrollHeight,
			winH: window.innerHeight,
			scrollH: (document.scrollingElement ?? document.documentElement).scrollHeight,
			scrollW: (document.scrollingElement ?? document.documentElement).scrollWidth,
			winW: window.innerWidth,
			docks: document.querySelectorAll('aside.dock').length,
			view: localStorage.getItem('cctui_list_view')
		};
	});
