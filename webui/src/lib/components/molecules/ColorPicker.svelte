<script lang="ts">
	// Hue picker (CCT-251): a trigger that opens a popover palette of hue swatches
	// plus an "Auto" (name-hash) option. Self-contained — owns its open state and
	// closes on selection. The `trigger` snippet renders whatever opens it (e.g. a
	// MachineBadge showing the current color); `value`/`onchange` carry the hue.
	import type { Snippet } from 'svelte';
	import Swatch from '$lib/components/atoms/Swatch.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		value = null,
		hues,
		disabled = false,
		label = m.misc_color_label(),
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
				<Swatch
					hue={null}
					active={value == null}
					title={m.misc_color_auto_title()}
					aria-label={m.misc_color_auto_label()}
					onclick={() => select(null)}>A</Swatch
				>
				{#each hues as h (h)}
					<Swatch
						hue={h}
						active={value === h}
						title={m.misc_hue_value({ hue: h })}
						aria-label={m.misc_hue_value({ hue: h })}
						onclick={() => select(h)}
					/>
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
</style>
