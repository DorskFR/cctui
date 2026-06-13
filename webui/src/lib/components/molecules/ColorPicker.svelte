<script lang="ts">
	// Hue picker (CCT-251): a trigger that opens a popover palette of hue swatches
	// plus an "Auto" (name-hash) option. Self-contained — owns its open state and
	// closes on selection. The `trigger` snippet renders whatever opens it (e.g. a
	// MachineBadge showing the current color); `value`/`onchange` carry the hue.
	import type { Snippet } from 'svelte';

	let {
		value = null,
		hues,
		disabled = false,
		label = 'Color',
		onchange,
		trigger
	}: {
		value?: number | null;
		hues: number[];
		disabled?: boolean;
		label?: string;
		onchange: (hue: number | null) => void;
		trigger: Snippet;
	} = $props();

	let open = $state(false);

	function select(hue: number | null) {
		onchange(hue);
		open = false;
	}
</script>

<span class="cp">
	<button
		class="cp-trigger"
		title={label}
		aria-label={label}
		aria-haspopup="true"
		aria-expanded={open}
		{disabled}
		onclick={() => (open = !open)}
	>
		{@render trigger()}
	</button>
	{#if open}
		<span class="cp-pop">
			<span class="row palette" role="radiogroup" aria-label={label}>
				<button
					class="swatch auto"
					class:active={value == null}
					title="Auto (name hash)"
					aria-label="Auto color"
					onclick={() => select(null)}>A</button
				>
				{#each hues as h (h)}
					<button
						class="swatch"
						class:active={value === h}
						style={`--sh:${h}`}
						title={`Hue ${h}`}
						aria-label={`Hue ${h}`}
						onclick={() => select(h)}
					></button>
				{/each}
			</span>
		</span>
	{/if}
</span>

<style>
	.cp {
		position: relative;
		display: inline-flex;
	}
	.cp-trigger {
		background: none;
		border: none;
		padding: 0;
		cursor: pointer;
		font: inherit;
	}
	.cp-trigger:disabled {
		cursor: default;
	}
	.cp-pop {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		z-index: 10;
	}
	.palette {
		gap: 4px;
		flex-wrap: wrap;
		width: max-content;
		max-width: 12rem;
		padding: var(--sp-2);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md, 6px);
		box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
	}
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
