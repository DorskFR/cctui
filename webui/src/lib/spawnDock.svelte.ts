import { MediaQuery } from 'svelte/reactivity';
import { settings, type SpawnDockSide } from './settings.svelte';
import { resolveDocks, type DockLayout } from './dock';
import { sessionsView } from './sessionsView.svelte';

export { SPAWN_DOCK_WIDTH, STATS_DOCK_WIDTH } from './dock';

// A docked panel only makes sense with room for a list beside it: below this
// width the settings are ignored and the "+ New" button / modal come back.
// Two panels on opposite edges need the wider breakpoint.
const wide = new MediaQuery('(min-width: 64rem)');
const veryWide = new MediaQuery('(min-width: 96rem)');

/** Set by the Sessions screen while a conversation is open: the spawn panel
 *  folds back into the "New session" button so one primary action shows. */
export const spawnDockHold = $state({ collapsed: false });

/** Where the spawn form, the stats panel and the conversation are docked right now, and the
 *  width the Sessions screen must keep clear on each edge. */
export function dockLayout(): DockLayout {
	return resolveDocks({
		spawn: spawnDockHold.collapsed ? { ...settings.spawnDock, enabled: false } : settings.spawnDock,
		stats: settings.statsDock,
		conversation: settings.conversationDock,
		wide: wide.current,
		veryWide: veryWide.current,
		tiles: sessionsView.tiles
	});
}

/** Which edge the spawn panel is docked to right now, or `null` when the
 *  Sessions screen should show the "+ New" button and modal instead (setting
 *  off, a conversation open, tiles mode, or the viewport is too narrow). */
export function spawnDockSide(): SpawnDockSide | null {
	return dockLayout().spawn;
}
