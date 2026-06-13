<script lang="ts">
	// Multi-line text input primitive. Owns its styling from theme tokens;
	// supports `bind:value` and passes through every native <textarea> attribute
	// and event (Svelte 5 events are props, so `...rest` forwards them). `mono`
	// switches to the monospace family. `autoresize` opts into the grow-with-
	// content action without the call-site wiring `use:` itself. `bind:el`
	// exposes the underlying element for focus/measure.
	import type { HTMLTextareaAttributes } from 'svelte/elements';
	import { autoresize as autoresizeAction } from '$lib/autoresize';

	type Props = HTMLTextareaAttributes & {
		mono?: boolean;
		autoresize?: boolean;
		class?: string;
		value?: HTMLTextareaAttributes['value'];
		el?: HTMLTextAreaElement | null;
	};

	let {
		mono = false,
		autoresize = false,
		class: klass = '',
		value = $bindable(),
		el = $bindable(null),
		...rest
	}: Props = $props();
</script>

{#if autoresize}
	<textarea
		bind:this={el}
		class="textarea {klass}"
		class:mono
		bind:value
		use:autoresizeAction={typeof value === 'string' ? value : ''}
		{...rest}
	></textarea>
{:else}
	<textarea bind:this={el} class="textarea {klass}" class:mono bind:value {...rest}></textarea>
{/if}

<style>
	.textarea {
		width: 100%;
		padding: var(--sp-3);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		color: var(--text);
		transition: border-color 0.12s var(--ease);
		resize: vertical;
		min-height: 5rem;
		font-family: inherit;
	}
	.textarea:focus {
		outline: none;
		border-color: var(--accent);
	}
	.mono {
		font-family: var(--font-mono);
	}
</style>
