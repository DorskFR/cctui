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

/** `chat`: set by the Sessions screen while a conversation is open; unless the
 *  user turned `foldOnChat` off, the spawn panel then minifies to its rail.
 *  `expanded`: the user opened the rail during this conversation. */
export const spawnDockHold = $state({ chat: false, expanded: false });

/** The spawn panel shows as its rail: kept minified by the user, or folded for a conversation. */
export function spawnDockMinified(): boolean {
	const dock = settings.spawnDock;
	const folded = spawnDockHold.chat && dock.foldOnChat !== false && !spawnDockHold.expanded;
	return dock.minified === true || folded;
}

/** Minify the spawn panel. During a conversation that folds it, this only
 *  re-folds it, so closing the conversation brings back the expanded form. */
export function minifySpawnDock(): void {
	if (spawnDockHold.chat && settings.spawnDock.foldOnChat !== false) spawnDockHold.expanded = false;
	else settings.setSpawnDock({ minified: true });
}

export function expandSpawnDock(): void {
	if (settings.spawnDock.minified) settings.setSpawnDock({ minified: false });
	if (spawnDockHold.chat) spawnDockHold.expanded = true;
}

/** Where the spawn form, the stats panel and the conversation are docked right now, and the
 *  width the Sessions screen must keep clear on each edge. */
export function dockLayout(): DockLayout {
	return resolveDocks({
		spawn: { ...settings.spawnDock, minified: spawnDockMinified() },
		stats: settings.statsDock,
		conversation: settings.conversationDock,
		wide: wide.current,
		veryWide: veryWide.current,
		tiles: sessionsView.tiles
	});
}

/** Which edge the spawn panel is docked to right now, or `null` when the
 *  Sessions screen should show the "+ New" button and modal instead (setting
 *  off, tiles mode, or the viewport is too narrow). */
export function spawnDockSide(): SpawnDockSide | null {
	return dockLayout().spawn;
}
