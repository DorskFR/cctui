import { MediaQuery } from 'svelte/reactivity';
import { settings, type SpawnDockSide } from './settings.svelte';

// Width of the docked spawn panel. Also the padding the Sessions screen's
// content reserves on that edge, so the two never drift apart.
export const SPAWN_DOCK_WIDTH = '30rem';

// The docked panel only makes sense with room for a list beside it: below this
// width the setting is ignored and the "+ New" button / modal come back.
const wide = new MediaQuery('(min-width: 64rem)');

/** Which edge the spawn panel is docked to right now, or `null` when the
 *  Sessions screen should show the "+ New" button and modal instead (setting
 *  off, or the viewport is too narrow for a side panel). */
export function spawnDockSide(): SpawnDockSide | null {
	const dock = settings.spawnDock;
	return dock.enabled && wide.current ? dock.side : null;
}
