/**
 * Auto-tiling grid geometry. Pure: the view owns the session ids, this owns
 * only the shape.
 *
 * The grid is picked from the pane count AND the viewport, not from
 * `ceil(sqrt(n))`: on a 32:9 ultrawide, 16 panes want 8 columns by 2 rows,
 * while the same 16 panes on 16:9 want 4 by 4. Each candidate `cols × rows`
 * is scored on how far a pane's aspect ratio lands from a readable target,
 * plus a penalty for cells the count cannot fill.
 *
 * A partial last row is widened to fill instead of leaving a hole, which needs
 * sub-tracks: the grid is laid out in `lcm` of the row counts so both a full
 * row and the short one divide it evenly. n=3 in a 2×2 gives the T layout
 * (two over one wide).
 */

export interface Viewport {
	width: number;
	height: number;
}

export interface TilePlacement {
	start: number;
	span: number;
	row: number;
}

export interface TileGrid {
	cols: number;
	rows: number;
}

export interface TileLayout extends TileGrid {
	/** Sub-track count: the value for `grid-template-columns: repeat(N, 1fr)`. */
	tracks: number;
	placements: TilePlacement[];
}

export const TARGET_PANE_RATIO = 4 / 3;
export const WASTE_WEIGHT = 1.5;

const FALLBACK: Viewport = { width: 1600, height: 900 };

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
 * Cost of laying `n` panes out as `cols × rows` in `viewport`: log-distance of
 * the pane ratio from `TARGET_PANE_RATIO`, plus the share of empty cells.
 * Logs so that halving and doubling the ratio cost the same.
 */
export function gridCost(n: number, cols: number, rows: number, viewport: Viewport): number {
	const cells = cols * rows;
	const ratio = viewport.width / cols / (viewport.height / rows);
	const distortion = Math.abs(Math.log(ratio / TARGET_PANE_RATIO));
	return distortion + WASTE_WEIGHT * ((cells - n) / cells);
}

/**
 * The columns × rows that fit `n` panes into `viewport` with the least
 * distortion. Columns that would leave a whole trailing column empty are
 * skipped, so every candidate is a grid the count can actually use.
 */
export function tileGrid(n: number, viewport: Viewport = FALLBACK): TileGrid {
	if (n <= 0) return { cols: 1, rows: 0 };
	const vp = viewport.width > 0 && viewport.height > 0 ? viewport : FALLBACK;
	let best: TileGrid = { cols: n, rows: 1 };
	let bestCost = Infinity;
	for (let cols = 1; cols <= n; cols++) {
		const rows = Math.ceil(n / cols);
		if ((cols - 1) * rows >= n) continue;
		const cost = gridCost(n, cols, rows, vp);
		if (cost < bestCost) {
			bestCost = cost;
			best = { cols, rows };
		}
	}
	return best;
}

/** Geometry for `n` tiles in `viewport`, with the last row widened to fill. */
export function tileLayout(n: number, viewport: Viewport = FALLBACK): TileLayout {
	if (n <= 0) return { cols: 1, rows: 0, tracks: 1, placements: [] };
	const { cols, rows } = tileGrid(n, viewport);
	const counts = rowCounts(n, cols);
	const tracks = counts.reduce((acc, c) => lcm(acc, c), 1);
	const placements: TilePlacement[] = [];
	counts.forEach((count, r) => {
		const span = tracks / count;
		for (let i = 0; i < count; i++) {
			placements.push({ start: i * span + 1, span, row: r + 1 });
		}
	});
	return { cols, rows, tracks, placements };
}

export const PANE_AREA = 950 * 460;
export const MAX_PANES = 10;

/**
 * How many panes the measured area is worth. Dividing by an area rather than by
 * a min width and a min height keeps the answer tied to how much screen a pane
 * actually gets: a 1080p window is worth 4, a 4K one at 150% is worth 8, an
 * ultrawide 10. Above the ceiling the grid stops growing — N live conversation
 * panes each cost a history fetch and a socket subscription, so an unbounded
 * count is what wedges the tab. Returns 0 for an unmeasured area so nothing
 * mounts before a size is known.
 */
export function paneCapacity(viewport: Viewport): number {
	if (!(viewport.width > 0 && viewport.height > 0)) return 0;
	const worth = Math.round((viewport.width * viewport.height) / PANE_AREA);
	return Math.min(Math.max(worth, 1), MAX_PANES);
}

/**
 * A pane's fixed chrome at tile widths: the compact one-row header (~44), the
 * activity line (27) and the folded one-line composer (~42), plus gaps and
 * borders — no meta row, no label strip, no filter bar. An estimate from those
 * parts, not a browser measurement; re-measure with the `tiles` Playwright
 * project when the pane gains or loses a row.
 */
export const MIN_PANE_CHROME = 140;
/**
 * Transcript that must be left over, or the pane is all chrome and no content.
 * A few lines, not a comfortable read: a tile is glanceable and scrolls. Any
 * higher and a real 1080p window — inner height ~960 once the browser chrome is
 * off, ~800 of tiles area under the bottom nav — loses its second row.
 */
export const MIN_TRANSCRIPT_HEIGHT = 96;
export const MIN_PANE_HEIGHT = MIN_PANE_CHROME + MIN_TRANSCRIPT_HEIGHT;

/**
 * How many of `n` panes to actually mount: what the area is worth, then dropped
 * one at a time while the grid the aspect-ratio choice lands on would leave a
 * pane too short to show any transcript. Never goes below one pane.
 */
export function fittingPaneCount(n: number, viewport: Viewport): number {
	const cap = paneCapacity(viewport);
	if (cap === 0 || n <= 0) return 0;
	let k = Math.min(n, cap);
	while (k > 1) {
		const { rows } = tileGrid(k, viewport);
		if (viewport.height / rows >= MIN_PANE_HEIGHT) break;
		k--;
	}
	return k;
}

/**
 * Reorder `next` to keep every id that was already placed where it was: a
 * session changing state must not move its tile. New ids land at the index the
 * sort gives them, gone ids leave a hole that closes.
 */
export function stableTileOrder(prev: string[], next: string[]): string[] {
	const wanted = new Set(next);
	const out = prev.filter((id) => wanted.has(id));
	const held = new Set(out);
	next.forEach((id, i) => {
		if (held.has(id)) return;
		out.splice(Math.min(i, out.length), 0, id);
		held.add(id);
	});
	return out;
}
