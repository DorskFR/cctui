// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { QueryClient } from '@tanstack/svelte-query';
import type { ServerEvent } from '@bindings/ServerEvent';
import { WsClient, type GithubEvent, type SessionEndedEvent } from './ws.svelte';
import { auth } from './auth.svelte';
import { qk } from './queries/keys';

// Fixtures mirror serde_json output: `Option::None` fields are omitted, not null.
const SID = 's1';
const ACCOUNT = '7d1c0a52-3f0e-4c47-9d8c-2b1e5f6a9c01';
const MACHINE = '0b6f2c1e-8a4d-4e2b-9f3a-5c7d1e2f3a4b';

const fixtures = {
	stream: {
		type: 'stream',
		session_id: SID,
		data: { type: 'text', content: '▷ User: hello  there', meta: false, ts: 1_700_000_000_000 }
	},
	status: { type: 'status', session_id: SID, status: 'active' },
	session_registered: {
		type: 'session_registered',
		session: {
			id: SID,
			parent_id: null,
			account_id: null,
			machine_id: MACHINE,
			working_dir: '/w',
			status: 'new',
			registered_at: '2026-01-01T00:00:00Z',
			last_heartbeat: '2026-01-01T00:00:00Z',
			metadata: {},
			adapter_id: 'claude-code'
		}
	},
	session_deregistered: { type: 'session_deregistered', session_id: SID },
	permission_request: {
		type: 'permission_request',
		session_id: SID,
		request_id: 'r1',
		tool_name: 'Bash',
		description: 'run ls',
		input_preview: 'ls'
	},
	permission_resolved: { type: 'permission_resolved', session_id: SID, request_id: 'r1' },
	ask_question: {
		type: 'ask_question',
		session_id: SID,
		question: 'Which one?',
		questions: [{ question: 'Which one?', header: 'Pick', options: [{ label: 'A' }], multiSelect: false }],
		preamble: 'Context first.'
	},
	ask_resolved: { type: 'ask_resolved', session_id: SID },
	plan_request: { type: 'plan_request', session_id: SID, plan: '# Plan' },
	plan_resolved: { type: 'plan_resolved', session_id: SID },
	command_result: { type: 'command_result', command_id: 'c1', ok: false, error: 'boom' },
	session_ended: { type: 'session_ended', session_id: SID, reason: 'crashed', detail: 'segfault' },
	message_ack: { type: 'message_ack', session_id: SID, client_msg_id: 'm1', ok: true },
	archive_manifest: { type: 'archive_manifest', machine_id: MACHINE, count: 3 },
	machine_liveness: { type: 'machine_liveness', machine_id: MACHINE, liveness: 'online' },
	machine_resources: {
		type: 'machine_resources',
		machine_id: MACHINE,
		resources: {
			cpu_pct: 25,
			mem_pct: 50,
			mem_used_bytes: 1024,
			mem_total_bytes: 2048,
			disk_pct: 10,
			disk_used_bytes: 10,
			disk_total_bytes: 100,
			disk_path: '/home/u'
		}
	},
	account_usage: {
		type: 'account_usage',
		account_id: ACCOUNT,
		usage: { windows: [], fetched_at: '2026-01-01T00:00:00Z' }
	},
	dispatcher_liveness: { type: 'dispatcher_liveness', dispatcher_id: MACHINE, liveness: 'stale' },
	archive_uploaded: {
		type: 'archive_uploaded',
		machine_id: MACHINE,
		project_dir: '-w',
		session_id: SID,
		size_bytes: 42,
		sha256: 'ab'.repeat(32)
	},
	github_event: {
		type: 'github_event',
		kind: 'pull',
		payload: { connector_id: 'gh1', repo: 'o/r', pull_number: 7 }
	},
	soft_limit_reached: {
		type: 'soft_limit_reached',
		session_id: SID,
		account_id: ACCOUNT,
		account_name: 'work',
		reason: 'five_hour at 95%',
		retry_after_secs: 600
	},
	soft_limit_cleared: { type: 'soft_limit_cleared', session_id: SID },
	tool_call_blocked: { type: 'tool_call_blocked', session_id: SID, tool_name: 'Bash', rule: 'rm -rf' },
	pty_chunk: { type: 'pty_chunk', session_id: SID, data: 'aGk=' },
	heartbeat: { type: 'heartbeat' },
	resync: { type: 'resync', session_id: SID }
} satisfies { [K in ServerEvent['type']]: Extract<ServerEvent, { type: K }> };

class FakeSocket {
	static OPEN = 1;
	readyState = 0;
	sent: Record<string, unknown>[] = [];
	onopen: (() => void) | null = null;
	onmessage: ((ev: { data: string }) => void) | null = null;
	onclose: (() => void) | null = null;
	onerror: (() => void) | null = null;
	constructor() {
		sockets.push(this);
	}
	send(raw: string) {
		this.sent.push(JSON.parse(raw));
	}
	close() {
		this.readyState = 3;
		this.onclose?.();
	}
	deliver(obj: ServerEvent) {
		this.onmessage?.({ data: JSON.stringify(obj) });
	}
}

let sockets: FakeSocket[] = [];
let realWs: unknown;

beforeEach(() => {
	sockets = [];
	realWs = (globalThis as Record<string, unknown>).WebSocket;
	(globalThis as Record<string, unknown>).WebSocket = FakeSocket;
	auth.isAuthed = true;
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
	(globalThis as Record<string, unknown>).WebSocket = realWs;
});

function setup() {
	const qc = new QueryClient();
	const c = new WsClient();
	c.bindQueryClient(qc);
	c.connect();
	const sock = sockets.at(-1)!;
	sock.readyState = 1;
	sock.onopen?.();
	return { c, sock, qc };
}

describe('onFrame, one fixture per ServerEvent variant', () => {
	it('stream appends the event and patches the list with the user excerpt', () => {
		const { c, sock } = setup();
		const seen: unknown[] = [];
		const patches: unknown[] = [];
		c.onStream(SID, (ev) => seen.push(ev));
		c.onListPatch((p) => patches.push(p));
		sock.deliver(fixtures.stream);
		expect(seen).toEqual([fixtures.stream.data]);
		expect(patches).toEqual([
			{
				session_id: SID,
				last_message_text: 'hello there',
				last_message_at: new Date(fixtures.stream.data.ts).toISOString()
			}
		]);
	});

	it.each(['status', 'session_registered', 'session_deregistered'] as const)(
		'%s bumps the list refresh tick',
		(kind) => {
			const { c, sock } = setup();
			const before = c.changeTick;
			sock.deliver(fixtures[kind]);
			vi.advanceTimersByTime(2000);
			expect(c.changeTick).toBe(before + 1);
		}
	);

	it('permission_request adds a prompt and permission_resolved removes it', () => {
		const { c, sock } = setup();
		let list: unknown[] = [];
		c.onPerms(SID, (l) => (list = l));
		sock.deliver(fixtures.permission_request);
		const { type: _, ...req } = fixtures.permission_request;
		expect(list).toEqual([req]);
		expect(c.pendingCount(SID)).toBe(1);
		sock.deliver(fixtures.permission_resolved);
		expect(list).toEqual([]);
	});

	it('ask_question sets the live ask and ask_resolved clears it', () => {
		const { c, sock } = setup();
		let ask: unknown = 'unset';
		c.onAsk(SID, (a) => (ask = a));
		sock.deliver(fixtures.ask_question);
		expect(ask).toEqual({
			question: 'Which one?',
			questions: fixtures.ask_question.questions,
			preamble: 'Context first.'
		});
		sock.deliver(fixtures.ask_resolved);
		expect(ask).toBeNull();
	});

	it('plan_request sets the live plan and plan_resolved clears it', () => {
		const { c, sock } = setup();
		let plan: unknown = 'unset';
		c.onPlan(SID, (p) => (plan = p));
		sock.deliver(fixtures.plan_request);
		expect(plan).toEqual({ plan: '# Plan', preamble: null });
		sock.deliver(fixtures.plan_resolved);
		expect(plan).toBeNull();
	});

	it('command_result resolves the awaiting command', async () => {
		const { c, sock } = setup();
		const out = c.awaitCommand('c1');
		sock.deliver(fixtures.command_result);
		await expect(out).resolves.toEqual({ ok: false, error: 'boom' });
	});

	it('session_ended notifies end listeners with the detail', () => {
		const { c, sock } = setup();
		const ends: SessionEndedEvent[] = [];
		c.onSessionEnded((ev) => ends.push(ev));
		sock.deliver(fixtures.session_ended);
		expect(ends).toEqual([{ session_id: SID, reason: 'crashed', detail: 'segfault' }]);
	});

	it('message_ack without a command id settles the tracked send', () => {
		const { c, sock } = setup();
		c.trackedSend(SID, 'hi', 1);
		const frame = sock.sent.find((f) => f.type === 'message');
		const clientMsgId = frame?.client_msg_id as string;
		expect(c.deliverySnapshot(SID).pending.has(1)).toBe(true);
		sock.deliver({ ...fixtures.message_ack, client_msg_id: clientMsgId });
		expect(c.deliverySnapshot(SID).pending.size).toBe(0);
	});

	it.each(['archive_manifest', 'machine_liveness', 'dispatcher_liveness', 'archive_uploaded', 'heartbeat'] as const)(
		'%s is accepted without side effects',
		(kind) => {
			const { c, sock } = setup();
			const before = c.changeTick;
			expect(() => sock.deliver(fixtures[kind])).not.toThrow();
			vi.advanceTimersByTime(2000);
			expect(c.changeTick).toBe(before);
		}
	);

	it('machine_resources reaches resource listeners', () => {
		const { c, sock } = setup();
		const seen: unknown[] = [];
		c.onMachineResources((ev) => seen.push(ev));
		sock.deliver(fixtures.machine_resources);
		expect(seen).toEqual([{ machine_id: MACHINE, resources: fixtures.machine_resources.resources }]);
	});

	it('account_usage reaches usage listeners', () => {
		const { c, sock } = setup();
		const seen: unknown[] = [];
		c.onAccountUsage((ev) => seen.push(ev));
		sock.deliver(fixtures.account_usage);
		expect(seen).toEqual([{ account_id: ACCOUNT, usage: fixtures.account_usage.usage }]);
	});

	it('github_event reaches inbox listeners', () => {
		const { c, sock } = setup();
		const seen: GithubEvent[] = [];
		c.onGithubEvent((ev) => seen.push(ev));
		sock.deliver(fixtures.github_event);
		expect(seen).toEqual([{ kind: 'pull', payload: fixtures.github_event.payload }]);
	});

	it('soft_limit_reached sets the block and soft_limit_cleared clears it', () => {
		const { c, sock } = setup();
		let sl: unknown = 'unset';
		c.onSoftLimit(SID, (v) => (sl = v));
		sock.deliver(fixtures.soft_limit_reached);
		expect(sl).toEqual({
			account_id: ACCOUNT,
			account_name: 'work',
			reason: 'five_hour at 95%',
			retry_after_secs: 600
		});
		sock.deliver(fixtures.soft_limit_cleared);
		expect(sl).toBeNull();
	});

	it('tool_call_blocked sets the tool block', () => {
		const { c, sock } = setup();
		let b: unknown = 'unset';
		c.onToolBlock(SID, (v) => (b = v));
		sock.deliver(fixtures.tool_call_blocked);
		expect(b).toEqual({ tool_name: 'Bash', rule: 'rm -rf' });
	});

	it('pty_chunk decodes base64 for terminal listeners', () => {
		const { c, sock } = setup();
		const chunks: Uint8Array[] = [];
		c.onPty(SID, (d) => chunks.push(d));
		sock.deliver(fixtures.pty_chunk);
		expect(Array.from(chunks[0])).toEqual([104, 105]);
	});

	it('resync invalidates the session conversation', () => {
		const { sock, qc } = setup();
		qc.setQueryData(qk.conversation(SID), []);
		sock.deliver(fixtures.resync);
		expect(qc.getQueryState(qk.conversation(SID))?.isInvalidated).toBe(true);
	});
});
