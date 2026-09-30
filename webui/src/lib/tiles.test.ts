import { describe, expect, it } from 'vitest';
import {
	MAX_PANES,
	fittingPaneCount,
	MIN_PANE_CHROME,
	MIN_PANE_HEIGHT,
	MIN_TRANSCRIPT_HEIGHT,
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

	// The four viewports the cap is specified against. Widths/heights are the
	// tiles area measured in the browser, not the window: the header and the
	// controls bar are already subtracted.
	it('is worth four panes on a 1080p window', () => {
		expect(paneCapacity({ width: 1912, height: 928 })).toBe(4);
	});

	it('is worth eight on a 4K screen at 150%', () => {
		expect(paneCapacity({ width: 2552, height: 1288 })).toBe(8);
	});

	it('is worth ten on a 3440x1440 ultrawide', () => {
		expect(paneCapacity({ width: 3432, height: 1288 })).toBe(10);
	});

	it('stops at the ceiling on a 4K screen at 100%', () => {
		expect(paneCapacity({ width: 3832, height: 2008 })).toBe(MAX_PANES);
	});

	it('never exceeds the ceiling, however much screen there is', () => {
		expect(paneCapacity({ width: 7680, height: 4320 })).toBe(MAX_PANES);
		expect(paneCapacity({ width: 100_000, height: 100_000 })).toBe(MAX_PANES);
	});

	it('always allows one pane, however cramped', () => {
		expect(paneCapacity({ width: 320, height: 200 })).toBe(1);
		expect(paneCapacity({ width: 1, height: 1 })).toBe(1);
		expect(paneCapacity({ width: 1912, height: 300 })).toBe(1);
	});

	it('grows monotonically with the area', () => {
		let last = 0;
		for (let w = 400; w <= 4000; w += 200) {
			const cap = paneCapacity({ width: w, height: 1000 });
			expect(cap).toBeGreaterThanOrEqual(last);
			last = cap;
		}
	});
});

describe('the grid the cap is laid out in', () => {
	const FHD = { width: 1912, height: 928 };
	const UW = { width: 3432, height: 1288 };

	it('lays a full 1080p cap out as 2x2', () => {
		expect(tileGrid(paneCapacity(FHD), FHD)).toEqual({ cols: 2, rows: 2 });
	});

	it('lays a full ultrawide cap out as 5x2', () => {
		expect(tileGrid(paneCapacity(UW), UW)).toEqual({ cols: 5, rows: 2 });
	});

	it('still picks by aspect ratio below the cap', () => {
		expect(tileGrid(1, FHD)).toEqual({ cols: 1, rows: 1 });
		expect(tileGrid(2, FHD)).toEqual({ cols: 2, rows: 1 });
		expect(tileGrid(3, FHD)).toEqual({ cols: 3, rows: 1 });
		expect(tileGrid(6, UW)).toEqual({ cols: 3, rows: 2 });
	});

	it('produces a grid that holds every pane the cap allows', () => {
		for (const vp of [FHD, UW, { width: 2552, height: 1288 }, { width: 1280, height: 600 }]) {
			const cap = paneCapacity(vp);
			for (let n = 1; n <= cap; n++) {
				const { cols, rows } = tileGrid(n, vp);
				expect(cols * rows, `n=${n} in ${vp.width}x${vp.height}`).toBeGreaterThanOrEqual(n);
			}
		}
	});
});

describe('the readable floor', () => {
	const FHD = { width: 1912, height: 928 };
	const UW = { width: 3432, height: 1288 };

	it('is the measured chrome plus a usable transcript', () => {
		expect(MIN_PANE_HEIGHT).toBe(MIN_PANE_CHROME + MIN_TRANSCRIPT_HEIGHT);
	});

	it('mounts nothing before the area has been measured', () => {
		expect(fittingPaneCount(8, { width: 0, height: 0 })).toBe(0);
		expect(fittingPaneCount(0, FHD)).toBe(0);
	});

	it('mounts the whole roster when it is under the cap', () => {
		expect(fittingPaneCount(3, FHD)).toBe(3);
		expect(fittingPaneCount(1, FHD)).toBe(1);
	});

	it('mounts no more than the area is worth', () => {
		expect(fittingPaneCount(24, FHD)).toBe(4);
		expect(fittingPaneCount(24, UW)).toBe(10);
	});

	it('keeps 2x2 on a real Full HD window, bottom nav included', () => {
		// A 1080p screen gives a browser ~960 of inner height, and the bottom nav
		// plus header and toolbar leave ~800 of tiles area.
		for (const h of [928, 875, 800]) {
			const vp = { width: 1912, height: h };
			expect(fittingPaneCount(24, vp), `${h}`).toBe(4);
			expect(tileGrid(4, vp), `${h}`).toEqual({ cols: 2, rows: 2 });
		}
	});

	it('gives a short area one full-height row instead of slivers', () => {
		// A short area gets one row, not two half-height ones: the ratio score
		// already prefers it, and every pane keeps the full height.
		for (const vp of [
			{ width: 2600, height: 650 },
			{ width: 1912, height: 380 }
		]) {
			const k = fittingPaneCount(24, vp);
			const { rows } = tileGrid(k, vp);
			expect(rows, `${vp.width}x${vp.height}`).toBe(1);
			expect(vp.height / rows).toBeGreaterThanOrEqual(MIN_PANE_HEIGHT);
		}
	});

	it('leaves every pane it does mount above the floor, or a single pane', () => {
		for (const vp of [FHD, UW, { width: 2552, height: 1288 }, { width: 3832, height: 2008 }]) {
			const k = fittingPaneCount(24, vp);
			const { rows } = tileGrid(k, vp);
			const where = `${vp.width}x${vp.height}`;
			expect(k, where).toBeGreaterThanOrEqual(1);
			if (k > 1) expect(vp.height / rows, where).toBeGreaterThanOrEqual(MIN_PANE_HEIGHT);
		}
	});

	it('never mounts more panes than asked for', () => {
		for (let n = 0; n <= 12; n++) {
			expect(fittingPaneCount(n, UW)).toBeLessThanOrEqual(n);
		}
	});
});
