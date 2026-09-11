<script lang="ts">
	// A ⋮⋮ reorder handle: grab it with a pointer (mouse, pen or touch) or focus
	// it and press the arrow keys. Subdued by default; the host row raises
	// `--grip-opacity` on hover so the handle never competes with the row label.
	let {
		label,
		hint,
		onmove,
		ongrab
	}: {
		label: string;
		hint?: string;
		onmove: (delta: -1 | 1) => void;
		ongrab?: (e: PointerEvent) => void;
	} = $props();
</script>

<button
	class="grip"
	type="button"
	aria-label={label}
	title={hint ?? label}
	onpointerdown={(e) => {
		// Touch implicitly captures the pointer to this button; hand it back so
		// the row under the finger receives the moves.
		const el = e.currentTarget as HTMLElement;
		if (el.hasPointerCapture?.(e.pointerId)) el.releasePointerCapture(e.pointerId);
		ongrab?.(e);
	}}
	onkeydown={(e) => {
		if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;
		e.preventDefault();
		onmove(e.key === 'ArrowUp' ? -1 : 1);
	}}
>
	<svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" aria-hidden="true">
		<circle cx="9" cy="5" r="1.6" />
		<circle cx="15" cy="5" r="1.6" />
		<circle cx="9" cy="12" r="1.6" />
		<circle cx="15" cy="12" r="1.6" />
		<circle cx="9" cy="19" r="1.6" />
		<circle cx="15" cy="19" r="1.6" />
	</svg>
</button>

<style>
	.grip {
		flex: none;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		padding: 0;
		width: 1.25rem;
		height: 1.5rem;
		border: 0;
		background: none;
		color: var(--text-faint);
		opacity: var(--grip-opacity, 0.4);
		cursor: grab;
		/* Own the gesture on a touch screen: without this the drag scrolls. */
		touch-action: none;
		transition: opacity var(--dur-fast, 120ms) ease;
	}
	.grip:hover,
	.grip:focus-visible {
		opacity: 1;
		color: var(--text-muted);
	}
	.grip:active {
		cursor: grabbing;
	}
</style>
