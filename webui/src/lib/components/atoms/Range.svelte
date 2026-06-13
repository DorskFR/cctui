<script lang="ts">
	// Range-slider primitive: the only place <input type="range"> is emitted.
	// Owns the full-width track + accent colour from tokens; supports `bind:value`
	// and passes through every native attribute (min/max/step/oninput/etc.).
	import type { HTMLInputAttributes } from 'svelte/elements';

	type Props = HTMLInputAttributes & {
		class?: string;
		value?: HTMLInputAttributes['value'];
	};

	let { class: klass = '', value = $bindable(), ...rest }: Props = $props();
</script>

<input type="range" class="range {klass}" bind:value {...rest} />

<style>
	.range {
		width: 100%;
		accent-color: var(--accent);
		cursor: pointer;
	}
</style>
