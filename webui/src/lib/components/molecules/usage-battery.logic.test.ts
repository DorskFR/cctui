import { describe, expect, it } from 'vitest';
import type { AccountUsageEntry, UsagePace, UsageWindow } from '$lib/queries';
import {
	aggregateBars,
	barPct,
	batteryBars,
	batteryEntries,
	countdown,
	headroomTone,
	paceState,
	wallInMs,
	worstPace
} from './usage-battery.logic';

const pace = (ratio: number, wall: string | null = null): UsagePace => ({
	elapsed_fraction: 0.5,
	expected_pct: 50,
	ratio,
	projected_wall_at: wall
});

const win = (key: string, utilization: number, p: UsagePace | null = null): UsageWindow => ({
	key,
	kind: key,
	label: key,
	utilization,
	resets_at: '2026-01-01T05:00:00Z',
	pace: p
});

const entry = (
	id: string,
	account: string,
	windows: UsageWindow[],
	header_pin = true
): AccountUsageEntry => ({
	account_id: id,
	provider: 'anthropic',
	usage: null,
	windows,
	age_secs: 0,
	account,
	account_name: account,
	account_emoji: null,
	header_pin
});

const now = Date.parse('2026-01-01T02:00:00Z');

describe('paceState', () => {
	it('leaf under 0.8, flame over 1.2, neutral between, nothing when unknown', () => {
		expect(paceState(pace(0.3))).toBe('leaf');
		expect(paceState(pace(0.8))).toBe('neutral');
		expect(paceState(pace(1.5))).toBe('flame');
		expect(paceState(null)).toBeNull();
	});
});

describe('wallInMs', () => {
	it('counts down to a wall that lands before the reset', () => {
		expect(wallInMs(pace(2, '2026-01-01T02:38:00Z'), '2026-01-01T05:00:00Z', now)).toBe(
			38 * 60_000
		);
	});
	it('is null when the wall is after the reset, or unknown', () => {
		expect(wallInMs(pace(2, '2026-01-01T06:00:00Z'), '2026-01-01T05:00:00Z', now)).toBeNull();
		expect(wallInMs(pace(0.2), '2026-01-01T05:00:00Z', now)).toBeNull();
		expect(wallInMs(pace(2, '2026-01-01T02:38:00Z'), null, now)).toBeNull();
	});
	it('clamps a wall already in the past to zero', () => {
		expect(wallInMs(pace(2, '2026-01-01T01:00:00Z'), '2026-01-01T05:00:00Z', now)).toBe(0);
	});
});

describe('countdown', () => {
	it('formats minutes, hours and days compactly', () => {
		expect(countdown(38 * 60_000)).toBe('38 min');
		expect(countdown(130 * 60_000)).toBe('2h10');
		expect(countdown(28 * 60 * 60_000)).toBe('1d 4h');
		expect(countdown(-5)).toBe('0 min');
	});
});

describe('headroomTone', () => {
	it('greens above 50% headroom, ambers to 20%, then reds', () => {
		expect(headroomTone(10)).toBe('ok');
		expect(headroomTone(60)).toBe('warn');
		expect(headroomTone(90)).toBe('danger');
		expect(headroomTone(null)).toBe('unknown');
		expect(headroomTone(Number.NaN)).toBe('unknown');
	});
});

describe('batteryBars / barPct', () => {
	it('picks the 5h and weekly windows by key', () => {
		const bars = batteryBars([win('weekly_all', 90), win('session', 12)]);
		expect(bars.fiveHour?.key).toBe('session');
		expect(bars.weekly?.key).toBe('weekly_all');
	});
	it('leaves a missing window null', () => {
		const bars = batteryBars([win('weekly_model:x', 70)]);
		expect(bars.fiveHour).toBeNull();
		expect(bars.weekly).toBeNull();
	});
	it('rounds and clamps the percentage', () => {
		expect(barPct(win('session', 33.6))).toBe(34);
		expect(barPct(win('session', 120))).toBe(100);
		expect(barPct(win('session', Number.NaN))).toBeNull();
		expect(barPct(null)).toBeNull();
	});
});

describe('worstPace', () => {
	it('picks the window burning hardest, ignoring ones without a pace', () => {
		expect(worstPace([win('session', 10, pace(0.4)), win('weekly_all', 20, pace(1.9))])?.key).toBe(
			'weekly_all'
		);
		expect(worstPace([win('session', 10), null])).toBeNull();
	});
});

describe('batteryEntries / aggregateBars', () => {
	const rows = [
		entry('p1', 'A', [win('session', 10), win('weekly_all', 20)]),
		entry('p2', 'A', [win('session', 80)]),
		entry('p3', 'B', []),
		entry('p4', 'C', [win('session', 95)], false)
	];

	it('keeps only pinned providers that reported a window', () => {
		expect(batteryEntries(rows).map((e) => e.providerId)).toEqual(['p1', 'p2']);
		expect(batteryEntries(null)).toEqual([]);
	});

	it('aggregates each bar to the fullest window across providers', () => {
		const agg = aggregateBars(batteryEntries(rows));
		expect(barPct(agg.fiveHour)).toBe(80);
		expect(barPct(agg.weekly)).toBe(20);
	});

	it('has nothing to aggregate when no provider is pinned', () => {
		const agg = aggregateBars([]);
		expect(agg.fiveHour).toBeNull();
		expect(agg.weekly).toBeNull();
	});
});
