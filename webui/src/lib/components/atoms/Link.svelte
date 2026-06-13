<script lang="ts">
	// Inline text link — underlined accent text with no button chrome. Renders an
	// <a> when `href` is given, otherwise a <button> (for in-page actions that
	// read as links, e.g. "Open ChatGPT again").
	import type { Snippet } from 'svelte';

	let {
		href,
		class: klass = '',
		children,
		...rest
	}: { href?: string; class?: string; children?: Snippet; [key: string]: unknown } = $props();
</script>

{#if href}
	<a {href} class="link {klass}" {...rest}>{@render children?.()}</a>
{:else}
	<button type="button" class="link {klass}" {...rest}>{@render children?.()}</button>
{/if}

<style>
	.link {
		background: none;
		border: none;
		padding: 0;
		color: var(--accent, var(--text));
		cursor: pointer;
		text-decoration: underline;
		font: inherit;
	}
</style>
