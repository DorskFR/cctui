<script lang="ts">
	// Text input primitive. Owns its styling from theme tokens; supports
	// `bind:value` and passes through every native <input> attribute. `mono`
	// switches to the monospace family (paths, tokens, env values).
	import type { HTMLInputAttributes } from 'svelte/elements';

	type Props = HTMLInputAttributes & {
		mono?: boolean;
		class?: string;
		value?: HTMLInputAttributes['value'];
		el?: HTMLInputElement | null;
	};

	let {
		mono = false,
		class: klass = '',
		value = $bindable(),
		el = $bindable(null),
		...rest
	}: Props = $props();
</script>

<input bind:this={el} class="input {klass}" class:mono bind:value {...rest} />

<style>
	.input {
		width: 100%;
		padding: var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		color: var(--text);
		transition: border-color 0.12s var(--ease);
	}
	.input:focus {
		outline: none;
		border-color: var(--accent);
	}
	.mono {
		font-family: var(--font-mono);
	}
</style>
