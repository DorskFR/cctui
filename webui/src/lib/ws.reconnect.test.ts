// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { QueryClient } from '@tanstack/svelte-query';
import type { AgentEvent } from '@bindings/AgentEvent';
import { WsClient, type PermReq } from './ws.svelte';
import { auth } from './auth.svelte';
import { qk } from './queries/keys';
import { endpoints } from './queries/endpoints';

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
	deliver(obj: unknown) {
		this.onmessage?.({ data: JSON.stringify(obj) });
	}
	open() {
		this.readyState = 1;
		this.onopen?.();
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
	vi.restoreAllMocks();
	(globalThis as Record<string, unknown>).WebSocket = realWs;
});

const SID = 's1';
const ev = (seq: number): AgentEvent => ({ type: 'text', content: `e${seq}`, meta: false, ts: seq, seq }) as AgentEvent;
const perm = (request_id: string) => ({
	type: 'permission_request',
	session_id: SID,
	request_id,
	tool_name: 'Bash',
	description: 'run',
	input_preview: 'ls',
	options: []
});

const lastSocket = () => {
	const sock = sockets.at(-1);
	if (!sock) throw new Error('no socket dialled');
	return sock;
};

function setup(cached: AgentEvent[] | undefined = [ev(100)]) {
	const qc = new QueryClient();
	if (cached) qc.setQueryData(qk.conversation(SID), cached);
	const c = new WsClient();
	c.bindQueryClient(qc);
	c.connect();
	lastSocket().open();
	c.subscribe(SID);
	const reconnect = () => {
		c.forceReconnect();
		const sock = lastSocket();
		sock.open();
		return sock;
	};
	return { c, qc, sock: lastSocket(), reconnect };
}

const flush = async () => {
	for (let i = 0; i < 4; i++) await Promise.resolve();
};

describe('reconnect catch-up', () => {
	it('fetches the events missed while the socket was down and delivers them before live ones', async () => {
		let land!: (evs: AgentEvent[]) => void;
		const fetch = vi
			.spyOn(endpoints, 'conversation')
			.mockImplementation(() => new Promise((res) => (land = res)));
		const { c, reconnect } = setup();
		const seen: (number | null | undefined)[] = [];
		c.onStream(SID, (e) => seen.push(e.seq));
		const sock = reconnect();
		expect(fetch).toHaveBeenCalledWith(SID, { after: 100 });
		sock.deliver({ type: 'stream', session_id: SID, data: ev(121) });
		expect(seen).toEqual([]);
		land([ev(101), ev(120)]);
		await flush();
		expect(seen).toEqual([101, 120, 121]);
	});

	it('does not fetch on the first open', () => {
		const fetch = vi.spyOn(endpoints, 'conversation').mockResolvedValue([]);
		setup();
		expect(fetch).not.toHaveBeenCalled();
	});

	it('leaves a session with no cached conversation to its query', () => {
		const fetch = vi.spyOn(endpoints, 'conversation').mockResolvedValue([]);
		const { reconnect } = setup(undefined);
		const sock = reconnect();
		expect(fetch).not.toHaveBeenCalled();
		expect(sock.sent).toContainEqual({ type: 'subscribe', session_id: SID });
	});

	it('rewinds the cache to the cursor when the catch-up fetch fails', async () => {
		vi.spyOn(endpoints, 'conversation').mockRejectedValue(new Error('offline'));
		const { qc, reconnect } = setup();
		const sock = reconnect();
		sock.deliver({ type: 'stream', session_id: SID, data: ev(121) });
		await flush();
		expect(qc.getQueryState(qk.conversation(SID))?.isInvalidated).toBe(true);
	});
});

describe('reconnect prompt reconciliation', () => {
	it('drops a permission resolved while the socket was down, keeping the replayed ones', () => {
		vi.spyOn(endpoints, 'conversation').mockResolvedValue([]);
		const { c, sock, reconnect } = setup();
		sock.deliver(perm('gone'));
		sock.deliver(perm('kept'));
		let perms: PermReq[] = [];
		c.onPerms(SID, (l) => (perms = l));
		expect(perms.map((p) => p.request_id)).toEqual(['gone', 'kept']);
		const next = reconnect();
		expect(perms).toEqual([]);
		next.deliver(perm('kept'));
		expect(perms.map((p) => p.request_id)).toEqual(['kept']);
		expect(c.pendingCount(SID)).toBe(1);
	});

	it('drops an ask and a plan the replay does not re-send', () => {
		vi.spyOn(endpoints, 'conversation').mockResolvedValue([]);
		const { c, sock, reconnect } = setup();
		sock.deliver({ type: 'ask_question', session_id: SID, question: 'Q?' });
		sock.deliver({ type: 'plan_request', session_id: SID, plan: 'P' });
		const asks: unknown[] = [];
		const plans: unknown[] = [];
		c.onAsk(SID, (a) => asks.push(a));
		c.onPlan(SID, (p) => plans.push(p));
		reconnect();
		expect(asks.at(-1)).toBeNull();
		expect(plans.at(-1)).toBeNull();
	});
});
