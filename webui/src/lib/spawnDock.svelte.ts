import { MediaQuery } from 'svelte/reactivity';
import { settings, type SpawnDockSide } from './settings.svelte';
import { resolveDocks, type DockLayout } from './dock';

export { SPAWN_DOCK_WIDTH, STATS_DOCK_WIDTH } from './dock';

// A docked panel only makes sense with room for a list beside it: below this
// width the settings are ignored and the "+ New" button / modal come back.
// Two panels on opposite edges need the wider breakpoint.
const wide = new MediaQuery('(min-width: 64rem)');
const veryWide = new MediaQuery('(min-width: 96rem)');

/** Where the spawn form and the stats panel are docked right now, and the
 *  width the Sessions screen must keep clear on each edge. */
export function dockLayout(): DockLayout {
	return resolveDocks({
		spawn: settings.spawnDock,
		stats: settings.statsDock,
		wide: wide.current,
		veryWide: veryWide.current
	});
}

/** Which edge the spawn panel is docked to right now, or `null` when the
 *  Sessions screen should show the "+ New" button and modal instead (setting
 *  off, or the viewport is too narrow for a side panel). */
export function spawnDockSide(): SpawnDockSide | null {
	return dockLayout().spawn;
}
