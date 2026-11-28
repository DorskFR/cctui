import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { StreamCatchUp, type CatchUpHost } from './catchup';

const ev = (seq: number): AgentEvent => ({ type: 'text', content: `e${seq}`, ts: seq, seq }) as AgentEvent;
const seqs = (list: AgentEvent[]) => list.map((e) => e.seq);

function harness(cursor: number | null) {
	let resolve!: (evs: AgentEvent[]) => void;
	let reject!: (e: unknown) => void;
	const delivered: AgentEvent[] = [];
	const host = {
		cursor: vi.fn(() => cursor),
		fetchAfter: vi.fn(
			() =>
				new Promise<AgentEvent[]>((res, rej) => {
					resolve = res;
					reject = rej;
				})
		),
		deliver: vi.fn((_sid: string, e: AgentEvent) => void delivered.push(e)),
		recover: vi.fn()
	} satisfies CatchUpHost;
	const c = new StreamCatchUp(host, 1000);
	return { c, host, delivered, resolve: (e: AgentEvent[]) => resolve(e), reject: (e: unknown) => reject(e) };
}

describe('StreamCatchUp', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it('fetches past the cursor captured at reconnect and delivers the gap before held live events', async () => {
		const h = harness(100);
		h.c.begin('s');
		expect(h.host.fetchAfter).toHaveBeenCalledWith('s', 100);
		expect(h.c.hold('s', ev(121))).toBe(true);
		expect(h.delivered).toEqual([]);
		h.resolve([ev(102), ev(101), ev(120)]);
		await Promise.resolve();
		await Promise.resolve();
		expect(seqs(h.delivered)).toEqual([101, 102, 120, 121]);
		expect(h.c.hold('s', ev(122))).toBe(false);
	});

	it('does not deliver a held live event twice when the fetch already carried it', async () => {
		const h = harness(100);
		h.c.begin('s');
		h.c.hold('s', ev(101));
		h.resolve([ev(101), ev(102)]);
		await Promise.resolve();
		await Promise.resolve();
		expect(seqs(h.delivered)).toEqual([101, 102]);
	});

	it('skips a session with nothing cached', () => {
		const h = harness(null);
		h.c.begin('s');
		expect(h.host.fetchAfter).not.toHaveBeenCalled();
		expect(h.c.hold('s', ev(1))).toBe(false);
	});

	it('reloads a cached conversation that has no seq to resume from', () => {
		const h = harness(0);
		h.c.begin('s');
		expect(h.host.fetchAfter).not.toHaveBeenCalled();
		expect(h.host.recover).toHaveBeenCalledWith('s', 0);
	});

	it('releases held events and rewinds when the fetch fails', async () => {
		const h = harness(100);
		h.c.begin('s');
		h.c.hold('s', ev(121));
		h.reject(new Error('offline'));
		await Promise.resolve();
		await Promise.resolve();
		expect(seqs(h.delivered)).toEqual([121]);
		expect(h.host.recover).toHaveBeenCalledWith('s', 100);
	});

	it('stops holding live events once the fetch times out', () => {
		const h = harness(100);
		h.c.begin('s');
		h.c.hold('s', ev(121));
		vi.advanceTimersByTime(1000);
		expect(seqs(h.delivered)).toEqual([121]);
		expect(h.host.recover).toHaveBeenCalledWith('s', 100);
		expect(h.c.hold('s', ev(122))).toBe(false);
	});

	it('keeps the first cursor across a second drop mid catch-up', async () => {
		const h = harness(100);
		h.c.begin('s');
		h.c.hold('s', ev(130));
		h.host.cursor.mockReturnValue(130);
		h.c.begin('s');
		expect(h.host.fetchAfter).toHaveBeenLastCalledWith('s', 100);
		h.resolve([ev(110)]);
		await Promise.resolve();
		await Promise.resolve();
		expect(seqs(h.delivered)).toEqual([110, 130]);
	});

	it('drops the gate on unsubscribe', async () => {
		const h = harness(100);
		h.c.begin('s');
		h.c.hold('s', ev(121));
		h.c.cancel('s');
		h.resolve([ev(101)]);
		await Promise.resolve();
		await Promise.resolve();
		expect(h.delivered).toEqual([]);
		expect(h.c.hold('s', ev(122))).toBe(false);
	});
});
