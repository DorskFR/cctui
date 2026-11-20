import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ReplyGate } from './replyGate';

function gate(delayMs = 800) {
	const changes: boolean[] = [];
	let delay = delayMs;
	const g = new ReplyGate({ delayMs: () => delay, onChange: (open) => changes.push(open) });
	return {
		g,
		changes,
		setDelay: (ms: number) => (delay = ms)
	};
}

describe('ReplyGate', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it('is open while nobody talks and lets a reply play at once', () => {
		const { g, changes } = gate();
		expect(g.open).toBe(true);
		expect(g.reply()).toBe(true);
		expect(g.userMessage()).toBe(false);
		expect(changes).toEqual([]);
	});

	it('never lets a reply play while the user is speaking', () => {
		const { g, changes } = gate();
		g.speechStart();
		expect(g.open).toBe(false);
		expect(g.reply()).toBe(false);
		vi.advanceTimersByTime(60_000);
		expect(g.open).toBe(false);
		expect(changes).toEqual([false]);
	});

	it('opens a held reply only once the grace delay after speech has passed', () => {
		const { g, changes } = gate(800);
		g.speechStart();
		g.reply();
		g.speechEnd();
		vi.advanceTimersByTime(799);
		expect(g.open).toBe(false);
		expect(changes).toEqual([false]);
		vi.advanceTimersByTime(1);
		expect(g.open).toBe(true);
		expect(changes).toEqual([false, true]);
	});

	it('speaking again inside the grace delay keeps the reply held', () => {
		const { g, changes } = gate(800);
		g.speechStart();
		g.speechEnd();
		vi.advanceTimersByTime(500);
		g.speechStart();
		vi.advanceTimersByTime(1000);
		expect(g.open).toBe(false);
		g.speechEnd();
		vi.advanceTimersByTime(800);
		expect(g.open).toBe(true);
		expect(changes).toEqual([false, true]);
	});

	it('reads the delay when speech ends, and zero opens at once', () => {
		const { g, setDelay } = gate(800);
		setDelay(2000);
		g.speechStart();
		g.speechEnd();
		vi.advanceTimersByTime(1999);
		expect(g.open).toBe(false);
		vi.advanceTimersByTime(1);
		expect(g.open).toBe(true);

		setDelay(0);
		g.speechStart();
		g.speechEnd();
		expect(g.open).toBe(true);
	});

	it('drops a reply the user talked over once their message goes out, only once', () => {
		const { g } = gate();
		g.speechStart();
		expect(g.reply()).toBe(false);
		g.speechEnd();
		expect(g.userMessage()).toBe(true);
		expect(g.userMessage()).toBe(false);
	});

	it('does not drop a reply that arrived after the user finished', () => {
		const { g } = gate(800);
		g.speechStart();
		g.speechEnd();
		vi.advanceTimersByTime(800);
		expect(g.reply()).toBe(true);
		expect(g.userMessage()).toBe(false);
	});

	it('forgets a talked-over reply once it has played out', () => {
		const { g } = gate(800);
		g.speechStart();
		g.reply();
		g.drained();
		g.speechEnd();
		vi.advanceTimersByTime(800);
		g.drained();
		expect(g.userMessage()).toBe(false);
	});

	it('a stray speech end does not reopen or schedule anything', () => {
		const { g, changes } = gate();
		g.speechEnd();
		vi.advanceTimersByTime(5000);
		expect(changes).toEqual([]);
	});

	it('reset opens a shut gate and forgets the held reply', () => {
		const { g, changes } = gate();
		g.speechStart();
		g.reply();
		g.reset();
		expect(g.open).toBe(true);
		expect(g.userMessage()).toBe(false);
		vi.advanceTimersByTime(5000);
		expect(changes).toEqual([false, true]);
		g.reset();
		expect(changes).toEqual([false, true]);
	});
});
