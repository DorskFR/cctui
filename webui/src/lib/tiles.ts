/**
 * Auto-tiling grid geometry. Pure: the workspace state owns the session ids,
 * this owns only the shape.
 *
 * `cols = ceil(sqrt(n))`, `rows = ceil(n / cols)`. A partial last row is
 * widened to fill instead of leaving a hole, which needs sub-tracks: the grid
 * is laid out in `lcm(cols, lastRowCount)` columns so both a full row and the
 * short one divide it evenly. n=3 gives the T layout (two over one wide), n=5
 * gives three over two widened.
 */

export type SplitDirection = 'vertical' | 'horizontal';

export interface TilePlacement {
	/** 1-based `grid-column-start`. */
	start: number;
	/** `grid-column-end` span, in sub-tracks. */
	span: number;
	/** 1-based `grid-row`. */
	row: number;
}

export interface TileLayout {
	/** Sub-track count: the value for `grid-template-columns: repeat(N, 1fr)`. */
	tracks: number;
	rows: number;
	placements: TilePlacement[];
}

function gcd(a: number, b: number): number {
	return b === 0 ? a : gcd(b, a % b);
}

function lcm(a: number, b: number): number {
	return (a * b) / gcd(a, b);
}

/** Tiles per row, top to bottom: full rows then whatever is left over. */
export function rowCounts(n: number, cols: number): number[] {
	const out: number[] = [];
	let left = n;
	while (left > 0) {
		out.push(Math.min(cols, left));
		left -= cols;
	}
	return out;
}

/**
 * Geometry for `n` tiles. `split` only matters at n = 2, where the pair is
 * either side by side (`vertical`, the default) or stacked (`horizontal`).
 * Above that the square-ish grid is the same either way, so transposing it
 * would just rotate a symmetric shape.
 */
export function tileLayout(n: number, split: SplitDirection = 'vertical'): TileLayout {
	if (n <= 0) return { tracks: 1, rows: 0, placements: [] };
	if (n === 1) return { tracks: 1, rows: 1, placements: [{ start: 1, span: 1, row: 1 }] };
	if (n === 2) {
		return split === 'horizontal'
			? {
					tracks: 1,
					rows: 2,
					placements: [
						{ start: 1, span: 1, row: 1 },
						{ start: 1, span: 1, row: 2 }
					]
				}
			: {
					tracks: 2,
					rows: 1,
					placements: [
						{ start: 1, span: 1, row: 1 },
						{ start: 2, span: 1, row: 1 }
					]
				};
	}

	const cols = Math.ceil(Math.sqrt(n));
	const counts = rowCounts(n, cols);
	const tracks = counts.reduce((acc, c) => lcm(acc, c), 1);
	const placements: TilePlacement[] = [];
	counts.forEach((count, r) => {
		const span = tracks / count;
		for (let i = 0; i < count; i++) {
			placements.push({ start: i * span + 1, span, row: r + 1 });
		}
	});
	return { tracks, rows: counts.length, placements };
}

export const TILES_MIN = 2;
export const TILES_MAX = 9;

export function clampMaxTiles(n: unknown): number {
	const v = typeof n === 'number' && Number.isFinite(n) ? Math.round(n) : 4;
	return Math.min(TILES_MAX, Math.max(TILES_MIN, v));
}

export function clampSplitDirection(v: unknown): SplitDirection {
	return v === 'horizontal' ? 'horizontal' : 'vertical';
}
