// Pure layout math for the panels docked to the edges of the Sessions screen
// (the spawn form and the stats panel). No Svelte state here so it can be unit
// tested; `spawnDock.svelte.ts` feeds it the settings and the media queries.
import type { SpawnDockSide } from './settings.svelte';

export type DockSide = SpawnDockSide;

// Width of each docked panel. Also the padding the Sessions screen's content
// reserves on that edge, so the two never drift apart.
export const SPAWN_DOCK_WIDTH = '30rem';
export const STATS_DOCK_WIDTH = '24rem';

export interface DockLayout {
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

export interface DockInputs {
	spawn: { enabled: boolean; side: DockSide };
	stats: { enabled: boolean; side: DockSide };
	/** Viewport wide enough for one docked column beside the list. */
	wide: boolean;
	/** Viewport wide enough for a docked column on each edge. */
	veryWide: boolean;
}

/** Resolve which panel goes where. A viewport too narrow for the requested
 *  panels drops them rather than squeezing the list: below `wide` nothing
 *  docks, and two panels on opposite edges need `veryWide` (the stats panel
 *  yields first since the spawn form is the one you type into). */
export function resolveDocks({ spawn, stats, wide, veryWide }: DockInputs): DockLayout {
	const none: DockLayout = { spawn: null, stats: null, stacked: false, left: null, right: null };
	if (!wide) return none;
	const spawnSide = spawn.enabled ? spawn.side : null;
	let statsSide = stats.enabled ? stats.side : null;
	if (spawnSide && statsSide && spawnSide !== statsSide && !veryWide) statsSide = null;
	const stacked = spawnSide !== null && spawnSide === statsSide;
	const widthOn = (side: DockSide): string | null => {
		if (spawnSide === side) return SPAWN_DOCK_WIDTH;
		if (statsSide === side) return STATS_DOCK_WIDTH;
		return null;
	};
	return { spawn: spawnSide, stats: statsSide, stacked, left: widthOn('left'), right: widthOn('right') };
}
