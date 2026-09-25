// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { QueryClient } from '@tanstack/svelte-query';
import { WsClient } from './ws.svelte';
import { auth } from './auth.svelte';
import { qk } from './queries/keys';

class FakeSocket {
	static OPEN = 1;
	readyState = 0;
	onopen: (() => void) | null = null;
	onmessage: ((ev: { data: string }) => void) | null = null;
	onclose: (() => void) | null = null;
	onerror: (() => void) | null = null;
	constructor() {
		sockets.push(this);
	}
	send() {}
	close() {
		this.readyState = 3;
		this.onclose?.();
	}
	deliver(obj: unknown) {
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
	vi.restoreAllMocks();
	(globalThis as Record<string, unknown>).WebSocket = realWs;
});

function setup() {
	const qc = new QueryClient();
	qc.setQueryData(qk.conversation('s1'), []);
	qc.setQueryData(qk.conversation('s2'), []);
	qc.setQueryData(qk.sessions(false), { sessions: [] });
	const c = new WsClient();
	c.bindQueryClient(qc);
	c.connect();
	const sock = sockets.at(-1)!;
	sock.readyState = 1;
	sock.onopen?.();
	const stale = (key: readonly unknown[]) => qc.getQueryState(key)?.isInvalidated;
	return { c, sock, stale };
}

describe('resync frame', () => {
	it('invalidates only the named conversation', () => {
		const { sock, stale } = setup();
		sock.deliver({ type: 'resync', session_id: 's1' });
		expect(stale(qk.conversation('s1'))).toBe(true);
		expect(stale(qk.conversation('s2'))).toBe(false);
		expect(stale(qk.sessions(false))).toBe(false);
	});

	it('without a session invalidates every conversation and the session list', () => {
		const { c, sock, stale } = setup();
		const tick = c.changeTick;
		sock.deliver({ type: 'resync' });
		expect(stale(qk.conversation('s1'))).toBe(true);
		expect(stale(qk.conversation('s2'))).toBe(true);
		expect(stale(qk.sessions(false))).toBe(true);
		vi.advanceTimersByTime(2000);
		expect(c.changeTick).toBe(tick + 1);
	});
});
