// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { QueryClient } from '@tanstack/svelte-query';
import { WsClient } from './ws.svelte';
import { auth } from './auth.svelte';

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
		this.sent.push(JSON.parse(raw) as Record<string, unknown>);
	}
	close() {
		this.readyState = 3;
		this.onclose?.();
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
	const c = new WsClient();
	c.bindQueryClient(new QueryClient());
	c.connect();
	const sock = sockets.at(-1)!;
	sock.readyState = 1;
	sock.onopen?.();
	return { c, sock };
}

const messages = (sock: FakeSocket) => sock.sent.filter((f) => f.type === 'message');

describe('the user-send frame', () => {
	it('Send here sends followup never', () => {
		const { c, sock } = setup();
		c.trackedSend('s1', 'stay here', 1, undefined, 't1', undefined, 'never');
		expect(messages(sock)).toHaveLength(1);
		expect(messages(sock)[0]).toMatchObject({ content: 'stay here', followup: 'never' });
		c.retryNow('s1', 1);
		expect(messages(sock).at(-1)).toMatchObject({ content: 'stay here', followup: 'never' });
	});

	it('a plain send carries followup auto', () => {
		const { c, sock } = setup();
		c.trackedSend('s1', 'hello', 1);
		c.trackedSend('s1', 'again', 2, undefined, 't2', undefined, 'auto');
		expect(messages(sock).map((f) => f.followup)).toEqual(['auto', 'auto']);
	});
});
