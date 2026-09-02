<script lang="ts">
	import type { DockSide } from '$lib/dock';
	import { dockResize } from '$lib/dockResize';
	import { m } from '$lib/paraglide/messages';

	// The thin drag grip on the INNER edge of a docked panel (spawn form / stats
	// panel): grab it with the mouse or a finger to change the panel's width,
	// double-click to go back to the default. The parent must be the panel
	// itself (positioned), since the grip is absolutely placed on its edge and
	// the drag reads the parent's width.
	let {
		side,
		onwidth,
		onreset
	}: { side: DockSide; onwidth: (px: number) => void; onreset?: () => void } = $props();
	let dragging = $state(false);
</script>

<div
	class="grip"
	class:grip-left={side === 'left'}
	class:dragging
	role="separator"
	aria-orientation="vertical"
	aria-label={m.dock_resize_grip()}
	title={m.dock_resize_grip()}
	use:dockResize={{ side, onwidth, onreset, ondrag: (a) => (dragging = a) }}
></div>

<style>
	/* A 10px hit area straddling the panel's border, with a 2px line that only
	   shows on hover / while dragging so the border stays quiet otherwise. */
	.grip {
		position: absolute;
		top: 0;
		bottom: 0;
		left: -5px;
		width: 10px;
		cursor: ew-resize;
		touch-action: none;
		z-index: 1;
	}
	.grip-left {
		left: auto;
		right: -5px;
	}
	.grip::after {
		content: '';
		position: absolute;
		top: 0;
		bottom: 0;
		left: 4px;
		width: 2px;
		background: var(--accent);
		opacity: 0;
		transition: opacity 0.12s var(--ease);
	}
	.grip:hover::after,
	.grip.dragging::after {
		opacity: 1;
	}
	/* Touch: no hover, so the line is faintly visible all the time. */
	@media (hover: none) {
		.grip::after {
			opacity: 0.35;
		}
	}
</style>
