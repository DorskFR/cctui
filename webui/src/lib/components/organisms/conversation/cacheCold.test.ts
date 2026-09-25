// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { CacheColdClock, COLD_WARN_MS } from './cacheCold.svelte';
import { ANTHROPIC_TTL_MS, DEFAULT_TTL_MS } from './cacheTtl';

const T0 = Date.UTC(2026, 0, 1, 12, 0, 0);

function clock(over: Partial<{ now: number; working: boolean; adapter: string; last: number | null }> = {}) {
	const now = over.now ?? T0;
	const last = over.last === undefined ? T0 : over.last;
	return new CacheColdClock({
		adapterId: () => over.adapter ?? 'claude-code',
		model: () => null,
		lastActivityAt: () => (last === null ? null : new Date(last).toISOString()),
		working: () => over.working ?? false,
		clock: () => now
	});
}

describe('CacheColdClock', () => {
	it('picks the TTL from the adapter family', () => {
		expect(clock().ttlMs).toBe(ANTHROPIC_TTL_MS);
		expect(clock({ adapter: 'other' }).ttlMs).toBe(DEFAULT_TTL_MS);
	});

	it('is warm right after the last turn', () => {
		const c = clock();
		expect(c.cold).toBe(false);
		expect(c.imminent).toBe(false);
		expect(c.countdownSecs).toBeNull();
		expect(c.msUntilCold).toBe(ANTHROPIC_TTL_MS);
	});

	it('goes cold once the TTL has elapsed', () => {
		const c = clock({ now: T0 + ANTHROPIC_TTL_MS + 1 });
		expect(c.cold).toBe(true);
		expect(c.imminent).toBe(false);
		expect(c.msUntilCold).toBeLessThan(0);
	});

	it('counts down through the final minute', () => {
		const c = clock({ now: T0 + ANTHROPIC_TTL_MS - 30_000 });
		expect(c.cold).toBe(false);
		expect(c.imminent).toBe(true);
		expect(c.countdownSecs).toBe(30);
		expect(clock({ now: T0 + ANTHROPIC_TTL_MS - COLD_WARN_MS - 1 }).imminent).toBe(false);
	});

	it('suppresses cold and countdown while a turn is in flight', () => {
		expect(clock({ now: T0 + ANTHROPIC_TTL_MS + 1, working: true }).cold).toBe(false);
		expect(clock({ now: T0 + ANTHROPIC_TTL_MS - 5_000, working: true }).imminent).toBe(false);
	});

	it('has no cache window without a last activity', () => {
		const c = clock({ last: null, now: T0 + ANTHROPIC_TTL_MS * 2 });
		expect(c.lastActivityMs).toBeNull();
		expect(c.cold).toBe(false);
		expect(c.msUntilCold).toBeNull();
	});
});
