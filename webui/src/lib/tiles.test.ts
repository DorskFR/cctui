import { describe, expect, it } from 'vitest';
import {
	clampMaxTiles,
	clampSplitDirection,
	rowCounts,
	tileLayout,
	TILES_MAX,
	TILES_MIN
} from './tiles';

/** Every tile's covered sub-track range, per row, so a layout can be checked
 *  for holes and overlaps rather than by eyeballing numbers. */
function coverage(n: number, split: 'vertical' | 'horizontal' = 'vertical') {
	const { tracks, rows, placements } = tileLayout(n, split);
	const perRow = new Map<number, number[]>();
	for (const p of placements) {
		const row = perRow.get(p.row) ?? new Array(tracks).fill(0);
		for (let t = p.start; t < p.start + p.span; t++) row[t - 1]++;
		perRow.set(p.row, row);
	}
	return { tracks, rows, perRow, placements };
}

describe('tileLayout', () => {
	it('fills every row completely, with no overlap, for 1…9 tiles', () => {
		for (const split of ['vertical', 'horizontal'] as const) {
			for (let n = 1; n <= 9; n++) {
				const { tracks, rows, perRow, placements } = coverage(n, split);
				expect(placements, `n=${n}`).toHaveLength(n);
				expect(perRow.size, `n=${n} rows`).toBe(rows);
				for (const [row, cells] of perRow) {
					expect(cells, `n=${n} ${split} row ${row}`).toEqual(new Array(tracks).fill(1));
				}
			}
		}
	});

	it('gives one tile the whole area', () => {
		expect(tileLayout(1)).toEqual({
			tracks: 1,
			rows: 1,
			placements: [{ start: 1, span: 1, row: 1 }]
		});
	});

	it('splits two side by side, or stacked on the horizontal preference', () => {
		const v = tileLayout(2, 'vertical');
		expect([v.tracks, v.rows]).toEqual([2, 1]);
		expect(v.placements.map((p) => p.row)).toEqual([1, 1]);

		const h = tileLayout(2, 'horizontal');
		expect([h.tracks, h.rows]).toEqual([1, 2]);
		expect(h.placements.map((p) => p.row)).toEqual([1, 2]);
	});

	it('widens the short last row instead of leaving a hole (n=3 is a T)', () => {
		const { tracks, rows, placements } = tileLayout(3);
		expect(rows).toBe(2);
		expect(placements[0].row).toBe(1);
		expect(placements[1].row).toBe(1);
		expect(placements[2]).toEqual({ start: 1, span: tracks, row: 2 });
		expect(placements[0].span).toBe(tracks / 2);
	});

	it('lays four out as a 2x2', () => {
		const { tracks, rows, placements } = tileLayout(4);
		expect([tracks, rows]).toEqual([2, 2]);
		expect(placements.map((p) => [p.start, p.row])).toEqual([
			[1, 1],
			[2, 1],
			[1, 2],
			[2, 2]
		]);
	});

	it('puts 5..9 on three columns, the partial last row widened', () => {
		for (let n = 5; n <= 9; n++) {
			expect(rowCounts(n, 3), `n=${n}`).toEqual(rowCounts(n, Math.ceil(Math.sqrt(n))));
			const { tracks, placements } = tileLayout(n);
			const lastRow = placements.filter((p) => p.row === Math.max(...placements.map((q) => q.row)));
			expect(lastRow[0].span, `n=${n}`).toBe(tracks / lastRow.length);
		}
		const five = tileLayout(5);
		expect(five.rows).toBe(2);
		expect(five.placements.filter((p) => p.row === 1)).toHaveLength(3);
		expect(five.placements.filter((p) => p.row === 2)).toHaveLength(2);
	});

	it('renders nothing for an empty workspace', () => {
		expect(tileLayout(0).placements).toEqual([]);
		expect(tileLayout(0).rows).toBe(0);
	});
});

describe('tiles clamps', () => {
	it('keeps maxTiles inside 2..9 and defaults to 4', () => {
		expect(clampMaxTiles(undefined)).toBe(4);
		expect(clampMaxTiles('nine')).toBe(4);
		expect(clampMaxTiles(0)).toBe(TILES_MIN);
		expect(clampMaxTiles(99)).toBe(TILES_MAX);
		expect(clampMaxTiles(6)).toBe(6);
		expect(clampMaxTiles(6.4)).toBe(6);
	});

	it('only accepts the two split directions', () => {
		expect(clampSplitDirection('horizontal')).toBe('horizontal');
		expect(clampSplitDirection('vertical')).toBe('vertical');
		expect(clampSplitDirection('sideways')).toBe('vertical');
		expect(clampSplitDirection(undefined)).toBe('vertical');
	});
});
