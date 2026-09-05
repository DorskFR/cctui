<script lang="ts">
	// Navigational/chrome anchor primitive: the only place a non-inline <a> is
	// emitted (the inline underlined text link is the separate Link atom). Owns a
	// minimal reset (no underline, inherits colour); consumers style the specific
	// look (bottom-nav item, version chip) via a passed class. Passes through
	// href/target/rel and every native attribute.
	import type { Snippet } from 'svelte';
	import type { HTMLAnchorAttributes } from 'svelte/elements';

	let {
		href,
		class: klass = '',
		children,
		...rest
	}: HTMLAnchorAttributes & { children?: Snippet } = $props();
</script>

<a {href} class="navlink {klass}" {...rest}>{@render children?.()}</a>

<style>
	.navlink {
		color: inherit;
		text-decoration: none;
		cursor: pointer;
	}
</style>
