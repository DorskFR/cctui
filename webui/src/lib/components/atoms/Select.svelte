<script lang="ts">
	// Native <select> primitive. Owns its styling from theme tokens; supports
	// `bind:value` and passes through every native attribute. Options are slotted
	// children so call-sites keep full control over <option> rendering.
	import type { Snippet } from 'svelte';
	import type { HTMLSelectAttributes } from 'svelte/elements';

	type Props = HTMLSelectAttributes & {
		class?: string;
		value?: HTMLSelectAttributes['value'];
		children?: Snippet;
	};

	let { class: klass = '', value = $bindable(), children, ...rest }: Props = $props();
</script>

<select class="select {klass}" bind:value {...rest}>
	{@render children?.()}
</select>

<style>
	.select {
		width: 100%;
		padding: var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		color: var(--text);
		transition: border-color 0.12s var(--ease);
	}
	.select:focus {
		outline: none;
		border-color: var(--accent);
	}
</style>
