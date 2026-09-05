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
} from './UsageBattery.logic';

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

const entry = (id: string, account: string, windows: UsageWindow[]): AccountUsageEntry => ({
	account_id: id,
	provider: 'anthropic',
	usage: null,
	windows,
	age_secs: 0,
	account,
	account_name: account
});

describe('headroomTone', () => {
	it('maps headroom to green / amber / red and unknown', () => {
		expect(headroomTone(10)).toBe('ok');
		expect(headroomTone(49)).toBe('ok');
		expect(headroomTone(50)).toBe('warn');
		expect(headroomTone(79)).toBe('warn');
		expect(headroomTone(80)).toBe('danger');
		expect(headroomTone(130)).toBe('danger');
		expect(headroomTone(null)).toBe('unknown');
		expect(headroomTone(Number.NaN)).toBe('unknown');
	});
});

describe('paceState', () => {
	it('leaf under 0.8, flame over 1.2, neutral between, nothing when unknown', () => {
		expect(paceState(pace(0.3))).toBe('leaf');
		expect(paceState(pace(0.8))).toBe('neutral');
		expect(paceState(pace(1.2))).toBe('neutral');
		expect(paceState(pace(1.5))).toBe('flame');
		expect(paceState(null)).toBeNull();
		expect(paceState(undefined)).toBeNull();
	});
});

describe('wallInMs', () => {
	const now = Date.parse('2026-01-01T02:00:00Z');
	it('counts down to a wall that lands before the reset', () => {
		const p = pace(2, '2026-01-01T02:38:00Z');
		expect(wallInMs(p, '2026-01-01T05:00:00Z', now)).toBe(38 * 60_000);
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
		expect(countdown(2 * 60 * 60_000)).toBe('2h00');
		expect(countdown(28 * 60 * 60_000)).toBe('1d 4h');
		expect(countdown(-5)).toBe('0 min');
	});
});

describe('batteryBars / barPct', () => {
	it('picks the 5h and weekly windows and rounds their fill', () => {
		const bars = batteryBars([win('weekly_all', 33.6), win('session', 120), win('weekly_model:x', 5)]);
		expect(barPct(bars.fiveHour)).toBe(100);
		expect(barPct(bars.weekly)).toBe(34);
	});
	it('reports a missing window as unknown', () => {
		const bars = batteryBars([win('session', 10)]);
		expect(bars.weekly).toBeNull();
		expect(barPct(bars.weekly)).toBeNull();
	});
});

describe('worstPace', () => {
	it('returns the window with the highest ratio, ignoring paceless ones', () => {
		const a = win('session', 10, pace(0.5));
		const b = win('weekly_all', 10, pace(1.4));
		expect(worstPace([win('x', 1), a, b])).toBe(b);
		expect(worstPace([win('x', 1)])).toBeNull();
	});
});

describe('batteryEntries / aggregateBars', () => {
	it('drops providers without windows and keeps account order', () => {
		const rows = [
			entry('p1', 'A', [win('session', 10)]),
			entry('p2', 'B', []),
			entry('p3', 'C', [win('weekly_all', 70)])
		];
		const entries = batteryEntries(rows);
		expect(entries.map((e) => e.providerId)).toEqual(['p1', 'p3']);
		expect(entries[0].accountName).toBe('A');
	});
	it('aggregates to the fullest window per bar', () => {
		const entries = batteryEntries([
			entry('p1', 'A', [win('session', 10), win('weekly_all', 90)]),
			entry('p2', 'B', [win('session', 60)])
		]);
		const agg = aggregateBars(entries);
		expect(agg.fiveHour?.utilization).toBe(60);
		expect(agg.weekly?.utilization).toBe(90);
		expect(aggregateBars([]).fiveHour).toBeNull();
	});
});
