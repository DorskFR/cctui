import { describe, it, expect } from 'vitest';
import type { UsageBucket } from '@bindings/UsageBucket';
import type { ModelUsage } from '@bindings/ModelUsage';
import type { HeatmapCell } from '@bindings/HeatmapCell';
import {
	fillBuckets,
	peakBucketTotal,
	rankModels,
	buildHeatGrid,
	hasUsage,
	bucketTotal,
	peakBucket,
	recentFrom,
	isAxisTick,
} from './usage-analytics';

// Anchor "now" to a fixed local wall-clock instant so bucket keys are stable.
const NOW = new Date(2026, 6, 15, 12, 30, 0); // 2026-07-15 12:30 local

const bucket = (d: Date, over: Partial<UsageBucket> = {}): UsageBucket => ({
	bucket: d.toISOString(),
	input: 0,
	output: 0,
	cache_read: 0,
	cache_creation: 0,
	...over,
});

describe('fillBuckets', () => {
	it('zero-fills a dense daily series and merges server rows by local day', () => {
		const today = new Date(2026, 6, 15, 9, 0, 0);
		const twoDaysAgo = new Date(2026, 6, 13, 22, 0, 0);
		const rows = [
			bucket(today, { input: 100, output: 10 }),
			bucket(twoDaysAgo, { input: 5, output: 1 }),
		];
		const filled = fillBuckets(rows, 7, 'day', NOW);
		expect(filled).toHaveLength(7); // one slot per day, no gaps
		// oldest→newest
		expect(filled[0].ms).toBeLessThan(filled[6].ms);
		// last slot = today, carries today's totals
		expect(filled[6].input).toBe(100);
		expect(filled[6].output).toBe(10);
		// the day between (yesterday) is a zero-fill
		expect(filled[5].input).toBe(0);
		// two-days-ago row landed on its slot
		expect(filled[4].input).toBe(5);
	});

	it('produces days*24 hourly slots for an hourly range', () => {
		const filled = fillBuckets([], 1, 'hour', NOW);
		expect(filled).toHaveLength(24);
		expect(filled.every((b) => b.input === 0)).toBe(true);
	});
});

describe('peakBucketTotal', () => {
	it('returns the max stacked height and never zero', () => {
		expect(peakBucketTotal([])).toBe(1);
		const filled = fillBuckets([bucket(NOW, { input: 3, output: 4, cache_read: 5 })], 1, 'day', NOW);
		expect(peakBucketTotal(filled)).toBe(12);
	});
});

describe('rankModels', () => {
	it('sorts by output desc and computes share + total', () => {
		const models: ModelUsage[] = [
			{ model: 'small', input: 1, output: 10, cache_read: 0, messages: 2 },
			{ model: 'big', input: 5, output: 50, cache_read: 5, messages: 9 },
		];
		const ranked = rankModels(models);
		expect(ranked[0].model).toBe('big');
		expect(ranked[0].share).toBe(1);
		expect(ranked[0].total).toBe(60);
		expect(ranked[1].share).toBeCloseTo(0.2);
	});
});

describe('buildHeatGrid', () => {
	it('builds a 7x24 grid, placing cells and tracking the max', () => {
		const cells: HeatmapCell[] = [
			{ dow: 0, hour: 0, messages: 3, output: 30 },
			{ dow: 6, hour: 23, messages: 9, output: 90 },
		];
		const { grid, maxMessages } = buildHeatGrid(cells);
		expect(grid).toHaveLength(7);
		expect(grid[0]).toHaveLength(24);
		expect(grid[0][0].messages).toBe(3);
		expect(grid[6][23].output).toBe(90);
		expect(grid[3][12].messages).toBe(0);
		expect(maxMessages).toBe(9);
	});

	it('ignores out-of-range cells', () => {
		const { maxMessages } = buildHeatGrid([{ dow: 9, hour: 40, messages: 100, output: 1 }]);
		expect(maxMessages).toBe(0);
	});
});

describe('peakBucket', () => {
	it('returns the heaviest bucket of the series', () => {
		const heavy = new Date(2026, 6, 10, 4, 0, 0);
		const rows = [
			bucket(new Date(2026, 6, 14), { input: 1 }),
			bucket(heavy, { input: 500, output: 100, cache_read: 400 }),
		];
		const peak = peakBucket(fillBuckets(rows, 7, 'day', NOW));
		expect(peak && bucketTotal(peak)).toBe(1000);
		expect(peak && new Date(peak.ms).getDate()).toBe(10);
	});

	it('is undefined for an empty series', () => {
		expect(peakBucket([])).toBeUndefined();
	});
});

describe('recentFrom', () => {
	it('marks the last five slots, and every slot in a shorter series', () => {
		expect(recentFrom(30)).toBe(25);
		expect(recentFrom(3)).toBe(0);
	});
});

describe('isAxisTick', () => {
	it('ticks every Nth slot counting back from the newest', () => {
		const ticks = Array.from({ length: 30 }, (_, i) => i).filter((i) => isAxisTick(i, 30, 7));
		expect(ticks).toEqual([1, 8, 15, 22, 29]);
	});
});

describe('hasUsage', () => {
	it('is false for undefined / all-empty and true when any series has data', () => {
		expect(hasUsage(undefined)).toBe(false);
		expect(hasUsage({ buckets: [], models: [], heatmap: [] })).toBe(false);
		expect(hasUsage({ buckets: [bucket(NOW)], models: [], heatmap: [] })).toBe(true);
	});
});
