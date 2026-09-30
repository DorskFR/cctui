import { describe, expect, it } from 'vitest';
import {
	MIN_PANE_HEIGHT,
	MIN_PANE_WIDTH,
	paneCapacity,
	rowCounts,
	stableTileOrder,
	tileGrid,
	tileLayout
} from './tiles';

const MONITOR = { width: 1920, height: 1080 };
const ULTRAWIDE = { width: 3840, height: 1080 };
const PORTRAIT = { width: 1080, height: 1920 };

describe('tileGrid', () => {
	it('gives one pane the whole window', () => {
		expect(tileGrid(1, MONITOR)).toEqual({ cols: 1, rows: 1 });
	});

	it('puts two panes side by side on a landscape monitor', () => {
		expect(tileGrid(2, MONITOR)).toEqual({ cols: 2, rows: 1 });
	});

	it('stacks two panes on a portrait viewport', () => {
		expect(tileGrid(2, PORTRAIT)).toEqual({ cols: 1, rows: 2 });
	});

	it('quadrants four panes', () => {
		expect(tileGrid(4, MONITOR)).toEqual({ cols: 2, rows: 2 });
	});

	it('keeps three panes in a 2x2 so none turns into a slit', () => {
		expect(tileGrid(3, MONITOR)).toEqual({ cols: 2, rows: 2 });
	});

	it('spreads 16 panes 8 wide by 2 on a 32:9 viewport', () => {
		expect(tileGrid(16, ULTRAWIDE)).toEqual({ cols: 8, rows: 2 });
	});

	it('squares the same 16 panes to 4x4 on 16:9', () => {
		expect(tileGrid(16, MONITOR)).toEqual({ cols: 4, rows: 4 });
	});

	it('never leaves a whole trailing column empty', () => {
		for (let n = 1; n <= 24; n++) {
			for (const vp of [MONITOR, ULTRAWIDE, PORTRAIT]) {
				const { cols, rows } = tileGrid(n, vp);
				expect(cols * rows, `${n} in ${vp.width}x${vp.height}`).toBeGreaterThanOrEqual(n);
				expect((cols - 1) * rows).toBeLessThan(n);
			}
		}
	});

	it('falls back to a sane grid before the viewport is measured', () => {
		expect(tileGrid(4, { width: 0, height: 0 })).toEqual({ cols: 2, rows: 2 });
	});

	it('has no shape for zero panes', () => {
		expect(tileGrid(0, MONITOR)).toEqual({ cols: 1, rows: 0 });
	});
});

describe('tileLayout', () => {
	it('widens a short last row instead of leaving a hole', () => {
		const l = tileLayout(3, MONITOR);
		expect(l.tracks).toBe(2);
		expect(l.placements).toEqual([
			{ start: 1, span: 1, row: 1 },
			{ start: 2, span: 1, row: 1 },
			{ start: 1, span: 2, row: 2 }
		]);
	});

	it('places every pane exactly once inside the grid', () => {
		const l = tileLayout(16, ULTRAWIDE);
		expect(l).toMatchObject({ cols: 8, rows: 2, tracks: 8 });
		expect(l.placements).toHaveLength(16);
		expect(l.placements.every((p) => p.start + p.span - 1 <= l.tracks)).toBe(true);
	});

	it('is empty with nothing to show', () => {
		expect(tileLayout(0, MONITOR).placements).toEqual([]);
	});
});

describe('rowCounts', () => {
	it('fills rows then the remainder', () => {
		expect(rowCounts(5, 3)).toEqual([3, 2]);
		expect(rowCounts(4, 2)).toEqual([2, 2]);
	});
});

describe('stableTileOrder', () => {
	it('keeps placed tiles put when the sort reshuffles them', () => {
		expect(stableTileOrder(['a', 'b', 'c'], ['c', 'b', 'a'])).toEqual(['a', 'b', 'c']);
	});

	it('closes the hole a removed session leaves', () => {
		expect(stableTileOrder(['a', 'b', 'c'], ['a', 'c'])).toEqual(['a', 'c']);
	});

	it('lands a new session at the index the sort gives it', () => {
		expect(stableTileOrder(['a', 'c'], ['a', 'b', 'c'])).toEqual(['a', 'b', 'c']);
		expect(stableTileOrder(['a', 'b'], ['n', 'a', 'b'])).toEqual(['n', 'a', 'b']);
	});

	it('seeds straight from the sort when nothing is placed', () => {
		expect(stableTileOrder([], ['a', 'b'])).toEqual(['a', 'b']);
	});
});

describe('paneCapacity', () => {
	it('mounts nothing before the area has been measured', () => {
		expect(paneCapacity({ width: 0, height: 0 })).toBe(0);
		expect(paneCapacity({ width: 1920, height: 0 })).toBe(0);
	});

	it('fits as many readable panes as the area allows', () => {
		// 1920/360 = 5 cols, 900/220 = 4 rows.
		expect(paneCapacity({ width: 1920, height: 900 })).toBe(20);
		// A 32:9 strip is wide but short: ten columns, four rows.
		expect(paneCapacity({ width: 3840, height: 900 })).toBe(40);
	});

	it('always allows one pane, however cramped', () => {
		expect(paneCapacity({ width: 320, height: 200 })).toBe(1);
		expect(paneCapacity({ width: 1, height: 1 })).toBe(1);
	});

	it('grows monotonically with the area', () => {
		let last = 0;
		for (let w = 400; w <= 4000; w += 200) {
			const cap = paneCapacity({ width: w, height: 1000 });
			expect(cap).toBeGreaterThanOrEqual(last);
			last = cap;
		}
	});

	it('never lets a pane fall below the readable minimum', () => {
		for (const vp of [
			{ width: 1920, height: 1080 },
			{ width: 3840, height: 1080 },
			{ width: 1080, height: 1920 },
			{ width: 1280, height: 720 }
		]) {
			const cap = paneCapacity(vp);
			const { cols, rows } = tileGrid(cap, vp);
			expect(vp.width / cols, `${vp.width}x${vp.height}`).toBeGreaterThanOrEqual(MIN_PANE_WIDTH);
			expect(vp.height / rows, `${vp.width}x${vp.height}`).toBeGreaterThanOrEqual(MIN_PANE_HEIGHT);
		}
	});
});
