// A fake cctui-daemon for the fixture machine: speaks the daemon WebSocket
// protocol so seeded sessions look alive (events stream, prompts can be
// answered from the UI, spawns land as new sessions). Everything it emits is
// invented acme data.
//
//   node site/demo/live-driver.mjs idle    keep fixture sessions registered
//   node site/demo/live-driver.mjs hero    play the hero story on a loop
import { randomUUID } from 'node:crypto';
import { API, MACHINE_KEY, S, sleep } from './lib.mjs';

export const S6 = 'a0000000-0000-4000-8000-000000000006';
export const DIR = '/work/acme/checkout-api';

export class FakeDaemon {
	constructor({ key = MACHINE_KEY, adapter = 'claude-code', verbose = false } = {}) {
		this.key = key;
		this.adapter = adapter;
		this.verbose = verbose;
		this.waiters = new Map();
		this.onSpawn = null;
		this.onReply = null;
		this.sessions = new Map();
	}

	async connect() {
		const url = API.replace(/^http/, 'ws') + '/api/v1/daemon/ws';
		this.ws = new WebSocket(url, { headers: { Authorization: `Bearer ${this.key}` } });
		await new Promise((resolve, reject) => {
			this.ws.addEventListener('open', resolve, { once: true });
			this.ws.addEventListener('error', (e) => reject(new Error(`daemon ws: ${e.message ?? 'error'}`)), { once: true });
		});
		this.ws.addEventListener('message', (m) => this.handle(JSON.parse(m.data)));
		this.ws.addEventListener('close', () => {
			if (this.verbose) console.log('daemon ws closed');
		});
		this.heartbeat();
		this.timer = setInterval(() => this.heartbeat(), 15000);
		return this;
	}

	close() {
		clearInterval(this.timer);
		this.ws?.close();
	}

	send(frame) {
		if (this.verbose) console.log('>>', JSON.stringify(frame).slice(0, 160));
		this.ws.send(JSON.stringify(frame));
	}

	heartbeat() {
		this.send({
			type: 'heartbeat',
			sent_at: new Date().toISOString(),
			harness: {
				versions: {},
				outcomes: [],
				binaries: {
					'claude-code': { present: true, version: '2.1.0' },
					codex: { present: true, version: '0.68.0' },
					opencode: { present: true, version: '1.2.4' }
				}
			}
		});
	}

	handle(frame) {
		if (this.verbose) console.log('<<', JSON.stringify(frame).slice(0, 300));
		if (frame.type !== 'command') return;
		const c = frame.command;
		if (c.kind === 'permission_response') {
			this.waiters.get(`perm:${c.request_id}`)?.(c);
		} else if (c.kind === 'reply') {
			this.event('message', { local_id: c.local_id, payload: { role: 'user', text: c.text }, turn_id: c.turn_id ?? undefined }, this.adapterOf(c.local_id));
			this.waiters.get(`reply:${c.local_id}`)?.(c);
			this.onReply?.(c, frame.adapter_id);
			if (c.command_id) this.event('command_result', { command_id: c.command_id, ok: true });
		} else if (c.kind === 'spawn' || c.kind === 'fork') {
			this.onSpawn?.(c, frame.adapter_id);
		} else if (c.kind === 'interrupt' || c.kind === 'rename' || c.kind === 'resume') {
			if (c.command_id) this.event('command_result', { command_id: c.command_id, ok: true });
		}
	}

	wait(key, timeoutMs = 120000) {
		return new Promise((resolve, reject) => {
			const t = setTimeout(() => {
				this.waiters.delete(key);
				reject(new Error(`timeout waiting for ${key}`));
			}, timeoutMs);
			this.waiters.set(key, (v) => {
				clearTimeout(t);
				this.waiters.delete(key);
				resolve(v);
			});
		});
	}

	event(kind, fields, adapter = this.adapter) {
		this.send({ type: 'event', adapter_id: adapter, event: { kind, ...fields } });
	}

	start(id, { working_dir = DIR, parent = null, adapter = this.adapter, spawn_key = null } = {}) {
		this.sessions.set(id, adapter);
		const extra = spawn_key ? { spawn_key } : {};
		this.event('session_started', { local_id: id, meta: { working_dir, parent_local_id: parent, extra } }, adapter);
	}

	adapterOf(id) {
		return this.sessions.get(id) ?? this.adapter;
	}

	status(id, fields) {
		this.event('status', { local_id: id, ...fields }, this.adapterOf(id));
	}

	title(id, title) {
		this.event('title', { local_id: id, title, source: 'agent' }, this.adapterOf(id));
	}

	msg(id, role, text, extra = {}) {
		this.event('message', { local_id: id, payload: { role, text, ...extra } }, this.adapterOf(id));
	}

	thinking(id, text) {
		this.msg(id, 'assistant_thinking', text);
	}

	say(id, text) {
		const message_id = `msg_${randomUUID().slice(0, 8)}`;
		this.msg(id, 'assistant', text, { message_id });
		return message_id;
	}

	tool(id, tool, input) {
		const toolId = `toolu_${randomUUID().slice(0, 8)}`;
		this.event('tool_use', { local_id: id, payload: { tool, input, id: toolId } }, this.adapterOf(id));
		return toolId;
	}

	result(id, toolId, content, is_error = false) {
		this.event('tool_use', { local_id: id, payload: { kind: 'tool_result', tool_use_id: toolId, content, is_error } }, this.adapterOf(id));
	}

	todos(id, todos) {
		this.event('todos', { local_id: id, todos }, this.adapterOf(id));
	}

	tokens(id, message_id, input_tokens, output_tokens, cache_read_tokens = 0, cache_creation_tokens = 0) {
		this.event('token_usage', { local_id: id, message_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens }, this.adapterOf(id));
	}

	turnEnd(id) {
		this.event('turn_end', { local_id: id }, this.adapterOf(id));
	}

	async permission(id, tool, input) {
		const request_id = randomUUID();
		const p = this.wait(`perm:${request_id}`);
		this.event('permission_request', { local_id: id, request_id, tool, input }, this.adapterOf(id));
		const answer = await p;
		this.event('permission_resolved', { local_id: id, request_id }, this.adapterOf(id));
		return answer;
	}

	async ask(id, question, questions, preamble) {
		const p = this.wait(`reply:${id}`);
		this.event('ask_question', { local_id: id, question, questions, preamble }, this.adapterOf(id));
		const reply = await p;
		this.event('ask_resolved', { local_id: id }, this.adapterOf(id));
		return reply;
	}

	async plan(id, plan, preamble) {
		const p = this.wait(`reply:${id}`);
		this.event('plan_request', { local_id: id, plan, preamble }, this.adapterOf(id));
		const reply = await p;
		this.event('plan_resolved', { local_id: id }, this.adapterOf(id));
		return reply;
	}

	end(id, kind = 'completed', detail) {
		this.event('session_ended', { local_id: id, reason: { kind, detail } }, this.adapterOf(id));
		this.sessions.delete(id);
	}
}

export const TODOS = (done, current) =>
	['Read the orders handler', 'Add a cursor parameter', 'Cover it with a test', 'Update the API reference'].map((content, i) => ({
		content,
		status: i < done ? 'completed' : i === current ? 'in_progress' : 'pending',
		active_form: content.replace(/^Read/, 'Reading').replace(/^Add/, 'Adding').replace(/^Cover/, 'Covering').replace(/^Update/, 'Updating')
	}));

export function registerFixture(d) {
	d.start(S.s1, { working_dir: DIR });
	d.start(S.s2, { working_dir: DIR });
	d.start(S.s5, { working_dir: DIR, parent: S.s1 });
	d.status(S.s1, { tempo: 'active', state: 'working', activity: 'Editing src/routes/orders.rs', name: 'Add pagination to the orders endpoint', model: 'opus', effort: 'medium' });
	d.status(S.s2, { tempo: 'blocked', state: 'waiting', activity: 'Waiting for an answer', name: 'Fix the flaky checkout test', model: 'sonnet' });
	d.status(S.s5, { tempo: 'active', state: 'working', activity: 'Running the orders test suite', name: 'Audit the orders endpoint tests', model: 'sonnet', effort: 'low' });
}

export function handleSpawns(d, { onStarted } = {}) {
	d.onSpawn = async (c) => {
		const spec = c.spec;
		const id = c.session_id ?? randomUUID();
		await sleep(900);
		d.start(id, { working_dir: spec.working_dir ?? DIR, parent: spec.parent_local_id ?? null, adapter: spec.adapter_id ?? 'claude-code', spawn_key: c.session_id ?? null });
		if (c.command_id) d.event('command_result', { command_id: c.command_id, ok: true });
		d.status(id, { tempo: 'active', state: 'working', activity: 'Reading the brief', name: spec.name ?? undefined, model: spec.model ?? undefined });
		if (spec.prompt) d.msg(id, 'user', spec.prompt);
		await sleep(1800);
		d.thinking(id, 'Starting from the brief. I will look at the existing tests before touching anything.');
		await sleep(1500);
		const t = d.tool(id, 'Glob', { pattern: 'tests/**/*.rs', path: spec.working_dir ?? DIR });
		await sleep(900);
		d.result(id, t, 'tests/orders.rs\ntests/checkout.rs\ntests/common/mod.rs');
		d.status(id, { activity: 'Reading tests/orders.rs' });
		onStarted?.(id);
	};
}

// The hero story on session 1: the agent writes a test, hits a permission
// prompt, carries on once it is approved, and spawns a subagent.
export async function heroStory(d, { pace = 1, onPermission, skipPermission = false } = {}) {
	const z = (ms) => sleep(ms * pace);
	const s1 = S.s1;
	d.status(s1, { tempo: 'active', state: 'working', activity: 'Writing tests/orders_pagination.rs' });
	d.thinking(s1, 'The two-page walk needs twelve orders so the second page is non-empty and the cursor from page one lands exactly on order seven.');
	await z(2200);
	const w = d.tool(s1, 'Write', {
		file_path: `${DIR}/tests/orders_pagination.rs`,
		content: '#[tokio::test]\nasync fn walks_two_pages() {\n    let app = test_app().await;\n    seed_orders(&app, 12).await;\n    let first: Page = get(&app, "/orders?limit=7").await;\n    assert_eq!(first.orders.len(), 7);\n    let second: Page = get(&app, &format!("/orders?limit=7&cursor={}", first.next_cursor.unwrap())).await;\n    assert_eq!(second.orders.len(), 5);\n    assert!(second.next_cursor.is_none());\n}\n'
	});
	await z(1400);
	d.result(s1, w, 'Wrote 11 lines to tests/orders_pagination.rs');
	await z(1200);
	d.say(s1, 'Test written. Running it against the real handler now.');
	d.status(s1, { activity: 'Running cargo test' });
	await z(1600);
	onPermission?.();
	const cmd = { command: 'cargo test -p checkout-api --test orders_pagination', description: 'Run the new pagination test' };
	const answer = skipPermission ? { allow: true } : await d.permission(s1, 'Bash', cmd);
	const t = d.tool(s1, 'Bash', { command: 'cargo test -p checkout-api --test orders_pagination', description: 'Run the new pagination test' });
	await z(1800);
	if (answer.allow === false) {
		d.result(s1, t, 'Permission denied by user', true);
		d.say(s1, 'Understood, I will not run the tests. Tell me when you want me to continue.');
		d.turnEnd(s1);
		return answer;
	}
	d.result(s1, t, 'running 1 test\ntest walks_two_pages ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; finished in 0.84s');
	await z(1300);
	d.todos(s1, TODOS(3, 3));
	const mid = d.say(s1, 'The two-page walk passes. I am handing the empty-page and stale-cursor cases to a subagent while I update the API reference.');
	d.tokens(s1, mid, 3800, 720, 92000, 0);
	d.status(s1, { activity: 'Updating docs/api/orders.md' });
	await z(1500);
	const spawn = d.tool(s1, 'mcp__cctui__CctuiAgent', { name: 'Edge cases for cursor pagination', prompt: 'Add tests for an empty page and a stale cursor on GET /orders. Report back with the test names.', model: 'sonnet' });
	await z(900);
	d.start(S6, { working_dir: DIR, parent: s1 });
	d.status(S6, { tempo: 'active', state: 'working', activity: 'Reading tests/orders_pagination.rs', name: 'Edge cases for cursor pagination', model: 'sonnet', effort: 'low' });
	d.msg(S6, 'user', 'Add tests for an empty page and a stale cursor on GET /orders. Report back with the test names.');
	await z(1600);
	d.thinking(S6, 'A stale cursor should return 400, not an empty page: the handler decodes it before querying.');
	await z(1400);
	const r = d.tool(S6, 'Read', { file_path: `${DIR}/tests/orders_pagination.rs` });
	await z(800);
	d.result(S6, r, '#[tokio::test]\nasync fn walks_two_pages() { … }  (11 lines)');
	d.status(S6, { activity: 'Writing the empty-page test' });
	await z(1200);
	d.result(s1, spawn, `Spawned subagent ${S6} (running)`);
	return answer;
}

export async function subagentStory(d, { pace = 1 } = {}) {
	const z = (ms) => sleep(ms * pace);
	const e = d.tool(S6, 'Edit', { file_path: `${DIR}/tests/orders_pagination.rs`, old_string: '}\n', new_string: '}\n\n#[tokio::test]\nasync fn empty_page_has_no_cursor() {\n    let app = test_app().await;\n    let page: Page = get(&app, "/orders?limit=7").await;\n    assert!(page.orders.is_empty());\n    assert!(page.next_cursor.is_none());\n}\n' });
	await z(1200);
	d.result(S6, e, 'Applied 1 edit to tests/orders_pagination.rs');
	d.todos(S6, [
		{ content: 'Empty page test', status: 'completed', active_form: 'Writing the empty page test' },
		{ content: 'Stale cursor test', status: 'in_progress', active_form: 'Writing the stale cursor test' }
	]);
	d.status(S6, { activity: 'Writing the stale-cursor test' });
	await z(1500);
	const b = d.tool(S5OrS6(d), 'Bash', { command: 'cargo test -p checkout-api orders::cursor', description: 'Run the cursor tests' });
	await z(1400);
	d.result(S5OrS6(d), b, 'running 3 tests\ntest cursor::round_trips ... ok\ntest cursor::rejects_garbage ... ok\ntest cursor::empty_page ... ok\n\ntest result: ok. 3 passed; 0 failed');
	d.status(S.s5, { activity: 'Checking the empty-page case' });
}

function S5OrS6(d) {
	return d.sessions.has(S.s5) ? S.s5 : S6;
}

// Session 2 asks a structured question and acts on the answer.
export async function askStory(d, { pace = 1 } = {}) {
	const z = (ms) => sleep(ms * pace);
	const s2 = S.s2;
	const reply = await d.ask(
		s2,
		'How should the checkout tests stop racing on the shared cart row?',
		[
			{
				question: 'How should the checkout tests stop racing on the shared cart row?',
				header: 'Fix',
				options: [
					{ label: 'Isolate the fixture', description: 'Each test seeds its own cart row (slower, no shared state)' },
					{ label: 'Serialise the module', description: 'Run checkout tests on one thread (fast to do, keeps the race latent)' }
				],
				multiSelect: false
			}
		],
		'Two tests share one seeded cart row, so they race.'
	);
	d.status(s2, { tempo: 'active', state: 'working', activity: 'Isolating the cart fixture' });
	d.thinking(s2, 'Isolating the fixture: give each test a cart created inside the test, keyed by the test name.');
	await z(1500);
	const t = d.tool(s2, 'Edit', { file_path: `${DIR}/tests/checkout.rs`, old_string: 'let cart = shared_cart();', new_string: 'let cart = fresh_cart(test_name!()).await;' });
	await z(1100);
	d.result(s2, t, 'Applied 1 edit to tests/checkout.rs');
	await z(900);
	d.say(s2, 'Each checkout test now seeds its own cart. Running the module twenty times to confirm the flake is gone.');
	return reply;
}

export async function runIdle(d) {
	registerFixture(d);
	handleSpawns(d);
	console.log('fake daemon idle; ctrl-c to stop');
	await new Promise(() => {});
}

if (process.argv[1]?.endsWith('live-driver.mjs')) {
	const mode = process.argv[2] ?? 'idle';
	const d = await new FakeDaemon({ verbose: process.env.VERBOSE === '1' }).connect();
	registerFixture(d);
	handleSpawns(d);
	if (mode === 'hero') {
		await heroStory(d, { onPermission: () => console.log('permission pending: answer it in the UI') });
		await subagentStory(d);
		console.log('hero story done');
	} else if (mode === 'ask') {
		await askStory(d);
	} else {
		await runIdle(d);
	}
	await sleep(1000);
	d.close();
}
