import { describe, expect, it } from 'vitest';
import type { UsageWindow } from '#lib/queries/index.ts';
import type { BatteryEntry } from './usage-battery.logic.ts';
import { entryReadings, gaugeTone, worstReading, type CapOf } from './usage-gauge.logic.ts';

const win = (key: string, utilization: number): UsageWindow => ({
	key,
	kind: key,
	label: key,
	utilization,
	resets_at: '2026-11-26T12:00:00Z'
});

const entry = (id: string, five: number | null, weekly: number | null): BatteryEntry => ({
	providerId: id,
	provider: 'claude',
	account: `acct-${id}`,
	accountName: id,
	bars: {
		fiveHour: five === null ? null : win('session', five),
		weekly: weekly === null ? null : win('weekly_all', weekly)
	},
	windows: []
});

const noCaps: CapOf = () => null;

describe('gaugeTone', () => {
	it('stays neutral below the default soft limit', () => {
		expect(gaugeTone(79, null)).toBe('ok');
		expect(gaugeTone(80, null)).toBe('warn');
	});

	it('turns amber past the configured soft limit, not before', () => {
		expect(gaugeTone(49, 50)).toBe('ok');
		expect(gaugeTone(50, 50)).toBe('warn');
		expect(gaugeTone(90, 95)).toBe('ok');
	});

	it('is danger once the window is spent', () => {
		expect(gaugeTone(100, 50)).toBe('danger');
	});
});

describe('entryReadings', () => {
	it('skips windows the provider did not report', () => {
		expect(entryReadings(entry('a', null, 40), noCaps).map((r) => r.window)).toEqual(['weekly_all']);
	});

	it('looks the cap up per provider and window', () => {
		const caps: CapOf = (id, w) => (id === 'a' && w === 'session' ? 30 : null);
		const [five, weekly] = entryReadings(entry('a', 35, 35), caps);
		expect(five.tone).toBe('warn');
		expect(weekly.tone).toBe('ok');
	});
});

describe('worstReading', () => {
	it('is null without readings', () => {
		expect(worstReading([])).toBeNull();
	});

	it('picks the fullest window when tones tie', () => {
		const rs = [...entryReadings(entry('a', 20, 88), noCaps), ...entryReadings(entry('b', 40, 10), noCaps)];
		expect(worstReading(rs)).toMatchObject({ window: 'weekly_all', pct: 88 });
	});

	it('prefers a window past its soft limit over a fuller one that is not', () => {
		const caps: CapOf = (id) => (id === 'b' ? 30 : 95);
		const rs = [...entryReadings(entry('a', 90, null), caps), ...entryReadings(entry('b', 35, null), caps)];
		expect(worstReading(rs)).toMatchObject({ pct: 35, tone: 'warn' });
	});
});
