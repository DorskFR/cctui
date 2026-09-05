import { describe, expect, it } from 'vitest';
import { capFromBar, capToBar, resetIn, usdPct, usdReadout, withCap } from './cap-bar.logic';

describe('cap ↔ bar', () => {
	it('treats 100 as no cap and rounds otherwise', () => {
		expect(capFromBar(100)).toBeNull();
		expect(capFromBar(72.6)).toBe(73);
		expect(capToBar(null)).toBe(100);
		expect(capToBar(80)).toBe(80);
		expect(capToBar(140)).toBe(100);
	});
});

describe('resetIn', () => {
	const now = Date.parse('2026-09-05T10:00:00Z');
	it('counts down to a future reset', () => {
		expect(resetIn('2026-09-05T13:00:00Z', now)).toBe('3h00');
		expect(resetIn('2026-09-05T10:20:00Z', now)).toBe('20 min');
	});
	it('is null when absent, past or unparseable', () => {
		expect(resetIn(null, now)).toBeNull();
		expect(resetIn('2026-09-05T09:00:00Z', now)).toBeNull();
		expect(resetIn('soon', now)).toBeNull();
	});
});

describe('dollar windows', () => {
	it('reads spend against the cap', () => {
		expect(usdPct(1.2, 5)).toBe(24);
		expect(usdPct(1.2, null)).toBeNull();
		expect(usdPct(9, 5)).toBe(100);
		expect(usdReadout(1.2, 5)).toBe('$1.20 / $5.00');
		expect(usdReadout(1.2, null)).toBe('$1.20');
		expect(usdReadout(null, 5)).toBeNull();
	});
});

describe('withCap', () => {
	it('sets the cap and keeps the bypass', () => {
		expect(withCap({ session: { cap_pct: 50, bypass_minutes: 10 } }, 'session', 75)).toEqual({
			session: { cap_pct: 75, bypass_minutes: 10 }
		});
	});
	it('adds a window and drops one left empty', () => {
		expect(withCap(null, 'weekly_all', 40)).toEqual({ weekly_all: { cap_pct: 40 } });
		expect(
			withCap({ session: { cap_pct: 50 }, weekly_all: { cap_pct: 1 } }, 'session', null)
		).toEqual({ weekly_all: { cap_pct: 1 } });
		expect(withCap({ session: { cap_pct: 50, bypass_minutes: 5 } }, 'session', null)).toEqual({
			session: { cap_pct: null, bypass_minutes: 5 }
		});
	});
});
