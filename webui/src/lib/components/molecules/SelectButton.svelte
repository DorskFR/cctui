<script lang="ts">
	// An icon/glyph button with a native <select> overlaid transparently on top:
	// it gets the platform dropdown UI while keeping the compact icon-button
	// affordance. Used by the header font-size + theme pickers and the drawer
	// font-size picker (CCT-250 #5 / CCT-297 #11). Composes the Button primitive
	// for the base look.
	import Button from '$lib/components/atoms/Button.svelte';

	let {
		glyph,
		label,
		title,
		value,
		options,
		onchange,
		class: klass = ''
	}: {
		// Visible content of the button (a letter like "A" or an emoji icon).
		glyph: string;
		label: string;
		title?: string;
		value: string;
		options: { value: string; label: string }[];
		onchange: (value: string) => void;
		class?: string;
	} = $props();
</script>

<Button variant="ghost" class="select-button btn-icon {klass}" {title} aria-label={label}>
	<span aria-hidden="true">{glyph}</span>
	<select
		class="overlay"
		aria-label={label}
		{value}
		onchange={(e) => onchange((e.currentTarget as HTMLSelectElement).value)}
	>
		{#each options as o (o.value)}
			<option value={o.value}>{o.label}</option>
		{/each}
	</select>
</Button>

<style>
	/* The button must clip + position the overlaid select. It's rendered by
	   Button (child component), so target it via :global. */
	:global(.select-button) {
		position: relative;
		overflow: hidden;
		font-weight: var(--fw-bold);
	}
	/* The select fills the button transparently — the glyph shows through. */
	.overlay {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		opacity: 0;
		cursor: pointer;
		border: none;
		background: none;
	}
</style>
