import { describe, it, expect } from 'vitest';
import type { TokenUsageWindows } from '@bindings/TokenUsageWindows';
import { buildWindowRows, WINDOW_KEYS } from './WindowsTable.logic';

const w = (input: number, output: number, cache_read: number) => ({ input, output, cache_read });

const windows: TokenUsageWindows = {
	hour: w(10, 5, 5),
	today: w(40, 20, 40),
	day: w(50, 25, 25),
	week: w(200, 100, 100),
	month: w(500, 250, 250)
};

describe('buildWindowRows', () => {
	it('emits one row per window in display order', () => {
		expect(buildWindowRows(windows).map((r) => r.key)).toEqual(WINDOW_KEYS);
	});

	it('scales the bar against the 30d total', () => {
		const rows = buildWindowRows(windows);
		const byKey = Object.fromEntries(rows.map((r) => [r.key, r]));
		expect(byKey.month.total).toBe(1000);
		expect(byKey.month.share).toBe(1);
		expect(byKey.hour.share).toBeCloseTo(0.02);
		expect(byKey.week.share).toBeCloseTo(0.4);
	});

	it('clamps a window that outgrew the 30d total', () => {
		const skewed = { ...windows, today: w(5000, 0, 0) };
		const today = buildWindowRows(skewed).find((r) => r.key === 'today');
		expect(today?.share).toBe(1);
	});

	it('returns zeroed rows with no share while the query is loading', () => {
		const rows = buildWindowRows(undefined);
		expect(rows).toHaveLength(5);
		expect(rows.every((r) => r.total === 0 && r.share === 0)).toBe(true);
	});
});
