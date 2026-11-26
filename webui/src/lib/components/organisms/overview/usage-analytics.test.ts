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
	axisLabel,
	axisTicks,
	rangeCost,
	bucketLabel,
} from './usage-analytics';

// Anchor "now" to a fixed local wall-clock instant so bucket keys are stable.
const NOW = new Date(2026, 6, 15, 12, 30, 0); // 2026-07-15 12:30 local

const bucket = (d: Date, over: Partial<UsageBucket> = {}): UsageBucket => ({
	bucket: d.toISOString(),
	input: 0,
	output: 0,
	cache_read: 0,
	cache_creation: 0,
	cost_usd: 0,
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

describe('rangeCost', () => {
	it("carries each row's dollars onto its slot and sums the range", () => {
		const today = new Date(2026, 6, 15, 9, 0, 0);
		const twoDaysAgo = new Date(2026, 6, 13, 22, 0, 0);
		const filled = fillBuckets(
			[bucket(today, { cost_usd: 1.25 }), bucket(twoDaysAgo, { cost_usd: 0.5 })],
			7,
			'day',
			NOW,
		);
		expect(filled[6].cost_usd).toBe(1.25);
		expect(filled[5].cost_usd).toBe(0);
		expect(filled[4].cost_usd).toBe(0.5);
		expect(rangeCost(filled)).toBeCloseTo(1.75);
	});

	it('reads a missing, negative or non-finite cost as nothing', () => {
		const rows = [
			bucket(NOW, { cost_usd: -1 }),
			{ ...bucket(NOW), cost_usd: Number.NaN },
			{ ...bucket(NOW), cost_usd: undefined as unknown as number },
		];
		for (const row of rows) {
			expect(rangeCost(fillBuckets([row], 1, 'day', NOW))).toBe(0);
		}
		expect(rangeCost([])).toBe(0);
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

describe('axisLabel', () => {
	const ms = new Date(2026, 10, 20, 14, 0, 0).getTime();
	it('labels a day bucket with month and day only', () => {
		expect(axisLabel(ms, 'day', 'en-US')).toBe('11/20');
		expect(axisLabel(ms, 'day', 'fr-FR')).toBe('20/11');
	});
	it('labels an hour bucket with the hour', () => {
		expect(axisLabel(ms, 'hour', 'en-US')).toBe('2:00 PM');
		expect(axisLabel(ms, 'hour', 'en-GB')).toBe('14:00');
	});
});

describe('axisTicks', () => {
	const overlaps = (ticks: { left: number }[], w: number, gap = 8) =>
		ticks.some((t, i) => i > 0 && t.left - ticks[i - 1].left - w < gap);

	it('labels every bar when they fit', () => {
		const ticks = axisTicks(7, 900, 36, 'day');
		expect(ticks.map((t) => t.index)).toEqual([0, 1, 2, 3, 4, 5, 6]);
	});

	it('thins a narrow 7-day axis, keeping the first and newest buckets', () => {
		const ticks = axisTicks(7, 400, 70, 'day');
		const idx = ticks.map((t) => t.index);
		expect(idx[0]).toBe(0);
		expect(idx.at(-1)).toBe(6);
		expect(idx.length).toBeLessThan(7);
		expect(overlaps(ticks, 70)).toBe(false);
	});

	it('never overlaps nor leaves the axis, at any width and range', () => {
		for (const [count, g] of [[24, 'hour'], [7, 'day'], [30, 'day']] as const) {
			for (let width = 60; width <= 1600; width += 7) {
				for (const lw of [30, 45, 60]) {
					const ticks = axisTicks(count, width, lw, g);
					expect(ticks.length).toBeGreaterThan(0);
					expect(ticks.at(-1)!.index).toBe(count - 1);
					expect(overlaps(ticks, Math.min(lw, width))).toBe(false);
					for (const t of ticks) {
						expect(t.left).toBeGreaterThanOrEqual(0);
						expect(t.left + Math.min(lw, width)).toBeLessThanOrEqual(width + 1e-9);
					}
				}
			}
		}
	});

	it('pins the edge labels inside the axis', () => {
		const ticks = axisTicks(30, 400, 40, 'day');
		expect(ticks[0]).toEqual({ index: 0, left: 0 });
		expect(ticks.at(-1)).toEqual({ index: 29, left: 360 });
	});

	it('waits for a measurement before labelling', () => {
		expect(axisTicks(7, 0, 40, 'day')).toEqual([]);
		expect(axisTicks(7, 400, 0, 'day')).toEqual([]);
	});
});

describe('hasUsage', () => {
	it('is false for undefined / all-empty and true when any series has data', () => {
		expect(hasUsage(undefined)).toBe(false);
		expect(hasUsage({ buckets: [], models: [], heatmap: [] })).toBe(false);
		expect(hasUsage({ buckets: [bucket(NOW)], models: [], heatmap: [] })).toBe(true);
	});
});

describe('bucketLabel', () => {
	const ms = new Date(2026, 10, 26, 14, 0, 0).getTime();
	it('names only the date of a day bucket', () => {
		expect(bucketLabel(ms, 'day', 'en-US')).toBe('11/26/2026');
		expect(bucketLabel(ms, 'day', 'ja-JP')).toBe('2026/11/26');
	});
	it('names the date and hour of an hour bucket, without seconds', () => {
		expect(bucketLabel(ms, 'hour', 'en-US')).toBe('11/26/2026, 2:00 PM');
		expect(bucketLabel(ms, 'hour', 'ja-JP')).toBe('2026/11/26 14:00');
	});
});
