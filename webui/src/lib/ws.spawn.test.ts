import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { SPAWN_ACK_TIMEOUT_MS, WsClient, spawnOutcomeFromEnd } from './ws.svelte';
import { auth } from './auth.svelte';

class FakeSocket {
	static CONNECTING = 0;
	static OPEN = 1;
	static CLOSING = 2;
	static CLOSED = 3;
	readyState = 0;
	onopen: (() => void) | null = null;
	onmessage: ((ev: { data: string }) => void) | null = null;
	onclose: (() => void) | null = null;
	onerror: (() => void) | null = null;
	constructor(public url: string) {
		sockets.push(this);
	}
	send() {}
	close() {
		this.readyState = 3;
		this.onclose?.();
	}
	accept() {
		this.readyState = 1;
		this.onopen?.();
	}
	deliver(obj: unknown) {
		this.onmessage?.({ data: JSON.stringify(obj) });
	}
}

let sockets: FakeSocket[] = [];
let realWs: unknown;
const last = (): FakeSocket => {
	const s = sockets.at(-1);
	if (!s) throw new Error('no socket');
	return s;
};

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

function openClient(): WsClient {
	const c = new WsClient();
	c.connect();
	last().accept();
	return c;
}

const settled = async <T>(p: Promise<T>): Promise<T | 'pending'> => {
	const r = await Promise.race([p, Promise.resolve('pending' as const)]);
	return r;
};

describe('awaitSpawn (CCT-971)', () => {
	it('resolves on the command_result ack as before', async () => {
		const c = openClient();
		const p = c.awaitSpawn('cmd-1', 'sess-1');
		last().deliver({ type: 'command_result', command_id: 'cmd-1', ok: true });
		expect(await p).toEqual({ ok: true });
	});

	it('closes on the pre-minted session registering before the (late) ack', async () => {
		const c = openClient();
		const p = c.awaitSpawn('cmd-1', 'sess-1');
		last().deliver({ type: 'session_registered', session: { id: 'sess-1' } });
		expect(await p).toEqual({ ok: true });
		last().deliver({ type: 'command_result', command_id: 'cmd-1', ok: false, error: 'late' });
		expect(await p).toEqual({ ok: true });
	});

	it('ignores events for other sessions', async () => {
		const c = openClient();
		const p = c.awaitSpawn('cmd-1', 'sess-1');
		last().deliver({ type: 'status', session_id: 'other', status: 'working' });
		last().deliver({ type: 'session_registered', session: { id: 'other' } });
		expect(await settled(p)).toBe('pending');
		last().deliver({ type: 'status', session_id: 'sess-1', status: 'working' });
		expect(await p).toEqual({ ok: true });
	});

	it('fails on a persisted spawn_failed end for that session', async () => {
		const c = openClient();
		const p = c.awaitSpawn('cmd-1', 'sess-1');
		last().deliver({
			type: 'session_ended',
			session_id: 'sess-1',
			reason: 'spawn_failed',
			detail: 'working_dir does not exist'
		});
		expect(await p).toEqual({ ok: false, error: 'working_dir does not exist' });
	});

	it('recovers a lost ack from the list probe (row present)', async () => {
		const c = openClient();
		const probe = vi.fn().mockResolvedValueOnce(null).mockResolvedValueOnce({ end_reason: null });
		const p = c.awaitSpawn('cmd-1', 'sess-1', { probe, probeIntervalMs: 1000 });
		await vi.advanceTimersByTimeAsync(1000);
		expect(probe).toHaveBeenCalledTimes(1);
		expect(await settled(p)).toBe('pending');
		await vi.advanceTimersByTimeAsync(1000);
		expect(probe).toHaveBeenCalledTimes(2);
		expect(await p).toEqual({ ok: true });
		await vi.advanceTimersByTimeAsync(10_000);
		expect(probe).toHaveBeenCalledTimes(2);
	});

	it('surfaces a failed-spawn row found by the probe as the error', async () => {
		const c = openClient();
		const probe = vi.fn().mockResolvedValue({ end_reason: 'spawn_failed', end_detail: 'boom' });
		const p = c.awaitSpawn('cmd-1', 'sess-1', { probe, probeIntervalMs: 1000 });
		await vi.advanceTimersByTimeAsync(1000);
		expect(await p).toEqual({ ok: false, error: 'boom' });
	});

	it('keeps probing past a probe error', async () => {
		const c = openClient();
		const probe = vi
			.fn()
			.mockRejectedValueOnce(new Error('offline'))
			.mockResolvedValueOnce({ end_reason: null });
		const p = c.awaitSpawn('cmd-1', 'sess-1', { probe, probeIntervalMs: 1000 });
		await vi.advanceTimersByTimeAsync(2000);
		expect(await p).toEqual({ ok: true });
	});

	it('stops probing once the ack lands', async () => {
		const c = openClient();
		const probe = vi.fn().mockResolvedValue(null);
		const p = c.awaitSpawn('cmd-1', 'sess-1', { probe, probeIntervalMs: 1000 });
		last().deliver({ type: 'command_result', command_id: 'cmd-1', ok: true });
		expect(await p).toEqual({ ok: true });
		await vi.advanceTimersByTimeAsync(5000);
		expect(probe).not.toHaveBeenCalled();
	});

	it('still ends unconfirmed at the timeout when nothing ever lands', async () => {
		const c = openClient();
		const probe = vi.fn().mockResolvedValue(null);
		const p = c.awaitSpawn('cmd-1', 'sess-1', { probe });
		await vi.advanceTimersByTimeAsync(SPAWN_ACK_TIMEOUT_MS);
		expect(await p).toMatchObject({ ok: false, timedOut: true });
	});

	it('falls back to the plain ack wait without a pre-minted id', async () => {
		const c = openClient();
		const p = c.awaitSpawn('cmd-1', null);
		last().deliver({ type: 'session_registered', session: { id: 'sess-1' } });
		expect(await settled(p)).toBe('pending');
		last().deliver({ type: 'command_result', command_id: 'cmd-1', ok: true });
		expect(await p).toEqual({ ok: true });
	});
});

describe('spawnOutcomeFromEnd', () => {
	it('treats a live or normally ended row as landed', () => {
		expect(spawnOutcomeFromEnd(null, null)).toEqual({ ok: true });
		expect(spawnOutcomeFromEnd('completed', null)).toEqual({ ok: true });
	});
	it('treats a failed start as the spawn error', () => {
		expect(spawnOutcomeFromEnd('spawn_failed', null)).toEqual({ ok: false, error: 'spawn_failed' });
		expect(spawnOutcomeFromEnd('resume_failed', 'x')).toEqual({ ok: false, error: 'x' });
	});
});
