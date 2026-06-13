<script lang="ts">
	// Native <select> primitive. Owns its styling from theme tokens; supports
	// `bind:value` and passes through every native attribute. Options are slotted
	// children so call-sites keep full control over <option> rendering.
	//
	// `variant="ghost"` makes the control fill its (positioned) parent fully
	// transparently — for the "native select overlaid on a custom trigger" pattern
	// (view picker, SelectButton): the platform popup with no custom outside-click
	// logic, while a styled trigger shows through underneath.
	import type { Snippet } from 'svelte';
	import type { HTMLSelectAttributes } from 'svelte/elements';

	type Props = HTMLSelectAttributes & {
		variant?: 'default' | 'ghost';
		class?: string;
		value?: HTMLSelectAttributes['value'];
		children?: Snippet;
	};

	let {
		variant = 'default',
		class: klass = '',
		value = $bindable(),
		children,
		...rest
	}: Props = $props();
</script>

<select class="select {variant} {klass}" bind:value {...rest}>
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
	/* Transparent overlay: fills the positioned parent, the trigger shows through. */
	.select.ghost {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		padding: 0;
		opacity: 0;
		border: none;
		background: none;
		cursor: pointer;
	}
</style>
