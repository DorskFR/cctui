<script lang="ts">
	// Colour-swatch button primitive: a round, selectable hue chip. `hue` is an
	// HSL hue (0–360) painted via `--sh`; pass `hue={null}` for the dashed "Auto"
	// chip (render its glyph as children). `active` draws the selection ring.
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';

	let {
		hue,
		active = false,
		class: klass = '',
		children,
		...rest
	}: HTMLButtonAttributes & {
		hue: number | null;
		active?: boolean;
		children?: Snippet;
	} = $props();
</script>

<button
	{...rest}
	type="button"
	class="swatch {hue == null ? 'auto' : ''} {active ? 'active' : ''} {klass}"
	style={hue == null ? undefined : `--sh:${hue}`}
>
	{@render children?.()}
</button>

<style>
	.swatch {
		width: 1.1rem;
		height: 1.1rem;
		border-radius: 50%;
		border: 1px solid transparent;
		background: hsl(var(--sh) 55% 40%);
		padding: 0;
		cursor: pointer;
		font-size: 0;
		transition: transform 0.1s var(--ease);
	}
	.swatch:hover {
		transform: scale(1.2);
	}
	.swatch.active {
		border-color: var(--text);
		box-shadow: 0 0 0 2px var(--bg);
	}
	.swatch.auto {
		background: var(--bg-elevated);
		border: 1px dashed var(--border-strong);
		color: var(--text-muted);
		font-size: var(--fs-xs);
		line-height: 1;
	}
	.swatch.auto.active {
		border-style: solid;
		border-color: var(--text);
	}
</style>
