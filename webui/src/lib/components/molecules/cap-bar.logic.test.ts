// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import {
	capFromBar,
	capToBar,
	isDenseWidth,
	resetIn,
	resetInShort,
	readoutColumns,
	USAGE_ROW_FIXED_REM,
	usdPct,
	usdReadout,
	withCap
} from './cap-bar.logic';

describe('cap ↔ bar', () => {
	it('treats 100 as no cap and rounds otherwise', () => {
		expect(capFromBar(100)).toBeNull();
		expect(capFromBar(72.6)).toBe(73);
		expect(capToBar(null)).toBe(100);
		expect(capToBar(80)).toBe(80);
		expect(capToBar(140)).toBe(100);
	});

	it('passes CapBar’s own uncapped null straight through', () => {
		expect(capFromBar(null)).toBeNull();
		expect(capFromBar(capToBar(capFromBar(null)))).toBeNull();
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

describe('resetInShort', () => {
	const now = Date.parse('2026-09-05T10:00:00Z');
	it('keeps only the largest unit', () => {
		expect(resetInShort('2026-09-05T10:20:00Z', now)).toBe('20m');
		expect(resetInShort('2026-09-05T13:30:00Z', now)).toBe('3h');
		expect(resetInShort('2026-09-10T22:00:00Z', now)).toBe('5d');
	});
	it('is null once the window has reset', () => {
		expect(resetInShort('2026-09-05T09:00:00Z', now)).toBeNull();
		expect(resetInShort(null, now)).toBeNull();
	});
});

describe('isDenseWidth', () => {
	const fixed = USAGE_ROW_FIXED_REM * 16;
	it('is dense only once a measured width drops under the threshold', () => {
		expect(isDenseWidth(0)).toBe(false);
		expect(isDenseWidth(300)).toBe(false);
		expect(isDenseWidth(299)).toBe(true);
	});
	it('keeps the row layout while the measured readout and a usable track fit', () => {
		expect(isDenseWidth(fixed + 90, 90)).toBe(false);
		expect(isDenseWidth(fixed + 89, 90)).toBe(true);
		expect(isDenseWidth(250, 80)).toBe(false);
	});
	it('scales the fixed part with the root font size', () => {
		expect(isDenseWidth(fixed + 90, 90, 20)).toBe(true);
		expect(isDenseWidth(USAGE_ROW_FIXED_REM * 20 + 90, 90, 20)).toBe(false);
	});
});

describe('readoutColumns', () => {
	it('gives every row of a stack the widest readout, rounded up', () => {
		const g = {};
		expect(
			readoutColumns([
				{ group: g, right: 300, width: 70.2 },
				{ group: g, right: 300.6, width: 95.4 },
				{ group: g, right: 300, width: 0 }
			])
		).toEqual([96, 96, 96]);
	});
	it('keeps side-by-side columns and separate groups apart', () => {
		const a = {};
		const b = {};
		expect(
			readoutColumns([
				{ group: a, right: 300, width: 70 },
				{ group: a, right: 600, width: 95 },
				{ group: b, right: 300, width: 120 },
				{ group: a, right: 299.5, width: 60 }
			])
		).toEqual([70, 95, 120, 70]);
	});
});
