import { describe, expect, it } from 'vitest';
import type { AccountUsageEntry, UsagePace, UsageWindow } from '$lib/queries';
import {
	busiest,
	countdown,
	gaugeEntries,
	gaugeGroups,
	gaugePace,
	gaugeWindow,
	paceState,
	utilizationPct,
	wallInMs
} from './header-gauges.logic';

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

describe('gaugePace', () => {
	it('flames when the projected wall lands before the reset', () => {
		expect(gaugePace(win('session', 80, pace(2, '2026-01-01T03:00:00Z')), now)).toBe('flame');
	});
	it('leafs under half the sustainable pace, and is silent on pace', () => {
		expect(gaugePace(win('session', 5, pace(0.4)), now)).toBe('leaf');
		expect(gaugePace(win('session', 40, pace(0.5)), now)).toBeNull();
		expect(gaugePace(win('session', 60, pace(1.4)), now)).toBeNull();
		expect(gaugePace(win('session', 60), now)).toBeNull();
		expect(gaugePace(null, now)).toBeNull();
	});
});

describe('gaugeWindow / utilizationPct', () => {
	it('prefers the 5h session window', () => {
		const w = gaugeWindow([win('weekly_all', 90), win('session', 12)]);
		expect(w?.key).toBe('session');
		expect(utilizationPct(w)).toBe(12);
	});
	it('falls back to the fullest window when there is no 5h one', () => {
		expect(gaugeWindow([win('weekly_all', 33.6), win('weekly_model:x', 70)])?.key).toBe(
			'weekly_model:x'
		);
		expect(gaugeWindow([])).toBeNull();
	});
	it('rounds and clamps the percentage', () => {
		expect(utilizationPct(win('session', 33.6))).toBe(34);
		expect(utilizationPct(win('session', 120))).toBe(100);
		expect(utilizationPct(win('session', Number.NaN))).toBeNull();
		expect(utilizationPct(null)).toBeNull();
	});
});

describe('gaugeEntries / gaugeGroups / busiest', () => {
	const rows = [
		entry('p1', 'A', [win('session', 10)]),
		entry('p2', 'A', [win('session', 80)]),
		entry('p3', 'B', []),
		entry('p4', 'C', [win('session', 95)], false)
	];

	it('keeps only pinned providers that reported a window', () => {
		expect(gaugeEntries(rows).map((e) => e.providerId)).toEqual(['p1', 'p2']);
		expect(gaugeEntries(null)).toEqual([]);
	});

	it('groups the cells of one account together', () => {
		const groups = gaugeGroups(gaugeEntries(rows));
		expect(groups).toHaveLength(1);
		expect(groups[0].accountName).toBe('A');
		expect(groups[0].entries.map((e) => e.providerId)).toEqual(['p1', 'p2']);
	});

	it('picks the most-used provider for the narrow strip', () => {
		expect(busiest(gaugeEntries(rows))?.providerId).toBe('p2');
		expect(busiest([])).toBeNull();
	});
});
