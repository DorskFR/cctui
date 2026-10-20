// Pure layout math for the panels docked to the edges of the Sessions screen
// (the spawn form, the stats panel and the conversation). No Svelte state here so it can be unit
// tested; `spawnDock.svelte.ts` feeds it the settings and the media queries.
import type { SpawnDockSide } from './settings.svelte';

export type DockSide = SpawnDockSide;

// Default width of each docked panel. Also the padding the Sessions screen's
// content reserves on that edge, so the two never drift apart. A panel's inner
// edge is a tsumikit resizeHandle grip: the width the user settles on is stored
// in px in the settings blob and wins over the default.
export const SPAWN_DOCK_WIDTH = '30rem';
export const STATS_DOCK_WIDTH = '24rem';
export const CONVERSATION_DOCK_WIDTH = '40rem';

// Bounds for a dragged width. The floor keeps the form usable; the ceiling is
// a sanity clamp on a stored value (the drag itself also stops at a share of
// the viewport so the list keeps room beside the panel).
export const DOCK_MIN_PX = 240;
export const DOCK_MAX_PX = 1600;
/** Largest share of the viewport a single dragged panel may take. */
export const DOCK_MAX_VIEWPORT_SHARE = 0.6;

/** Widest a dragged panel may get on this viewport: a share of the window,
 *  never below the floor. */
export function maxDockWidth(viewportWidth: number): number {
	return Math.max(DOCK_MIN_PX, Math.floor(viewportWidth * DOCK_MAX_VIEWPORT_SHARE));
}

/** Clamp a stored width to the drag bounds; anything that isn't a finite
 *  number means "not set" so the rem default applies. */
export function clampDockWidth(v: unknown): number | undefined {
	if (typeof v !== 'number' || !Number.isFinite(v)) return undefined;
	return Math.min(DOCK_MAX_PX, Math.max(DOCK_MIN_PX, Math.round(v)));
}

/** A width persisted as a localStorage string; `null` (never stored) is "not set",
 *  not `Number(null) === 0` clamped up to the minimum. */
export function storedDockWidth(raw: string | null): number | undefined {
	return raw === null || raw === '' ? undefined : clampDockWidth(Number(raw));
}

export interface DockLayout {
	/** Edge the conversation is pinned to, or `null` for the overlay drawer. */
	conversation: DockSide | null;
	/** Edge the spawn form is pinned to, or `null` for the "+ New" button + modal. */
	spawn: DockSide | null;
	/** Edge the stats panel is pinned to, or `null` when hidden. */
	stats: DockSide | null;
	/** Both panels share an edge: the spawn form takes the top half of that
	 *  column and the stats panel the bottom half, at the spawn panel's width. */
	stacked: boolean;
	/** Width the content must keep clear on each edge (`null` = nothing docked). */
	left: string | null;
	right: string | null;
}

export interface DockRequest {
	enabled: boolean;
	side: DockSide;
	width?: number;
}

export interface DockInputs {
	spawn: DockRequest;
	stats: DockRequest;
	/** Conversation pinned beside the list instead of the overlay drawer. */
	conversation?: DockRequest;
	/** Viewport wide enough for one docked column beside the list. */
	wide: boolean;
	/** Viewport wide enough for a docked column on each edge. */
	veryWide: boolean;
	/** Tiles mode: the panes own the whole viewport, so nothing docks. */
	tiles?: boolean;
}

/** Resolve which panel goes where. A viewport too narrow for the requested
 *  panels drops them rather than squeezing the list: below `wide` nothing
 *  docks, and two panels on opposite edges need `veryWide` (the stats panel
 *  yields first since the spawn form is the one you type into). Tiles drop
 *  every panel, leaving the settings untouched so list/grid get them back
 *  unchanged.
 *
 *  A docked conversation owns its edge outright: a spawn form or stats panel
 *  asked for the same edge moves to the opposite one, where the two stack as
 *  usual. The conversation is the one you came for, so below `veryWide` it is
 *  the other panels that yield and the "+ New" button comes back. */
export function resolveDocks({
	spawn,
	stats,
	conversation,
	wide,
	veryWide,
	tiles
}: DockInputs): DockLayout {
	const none: DockLayout = {
		conversation: null,
		spawn: null,
		stats: null,
		stacked: false,
		left: null,
		right: null
	};
	if (tiles || !wide) return none;
	const convSide = conversation?.enabled ? conversation.side : null;
	const awayFromConv = (side: DockSide): DockSide =>
		side === convSide ? (side === 'left' ? 'right' : 'left') : side;
	let spawnSide = spawn.enabled ? awayFromConv(spawn.side) : null;
	let statsSide = stats.enabled ? awayFromConv(stats.side) : null;
	if (convSide && !veryWide) {
		spawnSide = null;
		statsSide = null;
	}
	if (spawnSide && statsSide && spawnSide !== statsSide && !veryWide) statsSide = null;
	const stacked = spawnSide !== null && spawnSide === statsSide;
	const px = (w: number | undefined, fallback: string) => {
		const c = clampDockWidth(w);
		return c === undefined ? fallback : `${c}px`;
	};
	// A stacked column is sized by the spawn panel (the one you type into).
	const widthOn = (side: DockSide): string | null => {
		if (convSide === side) return px(conversation?.width, CONVERSATION_DOCK_WIDTH);
		if (spawnSide === side) return px(spawn.width, SPAWN_DOCK_WIDTH);
		if (statsSide === side) return px(stats.width, STATS_DOCK_WIDTH);
		return null;
	};
	return {
		conversation: convSide,
		spawn: spawnSide,
		stats: statsSide,
		stacked,
		left: widthOn('left'),
		right: widthOn('right')
	};
}
