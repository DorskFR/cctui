// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { health } = vi.hoisted(() => ({ health: { reportFailure: vi.fn(), reportOk: vi.fn() } }));
vi.mock('./backendHealth.svelte', async (orig) => ({
	...(await orig<typeof import('./backendHealth.svelte')>()),
	backendHealth: health
}));

import { WsClient } from './ws.svelte';
import { auth } from './auth.svelte';

class FakeSocket {
	static OPEN = 1;
	readyState = 0;
	onopen: (() => void) | null = null;
	onmessage: (() => void) | null = null;
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
	vi.clearAllMocks();
	(globalThis as Record<string, unknown>).WebSocket = realWs;
});

describe('websocket feeds backend health', () => {
	it('a socket that cannot connect reports a failure', () => {
		const c = new WsClient();
		c.connect();
		sockets[0].onerror?.();
		expect(health.reportFailure).toHaveBeenCalledTimes(1);
		c.disconnect();
	});

	it('a deliberate disconnect does not', () => {
		const c = new WsClient();
		c.connect();
		c.disconnect();
		expect(health.reportFailure).not.toHaveBeenCalled();
	});
});
