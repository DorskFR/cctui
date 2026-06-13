<script lang="ts">
	// Elevated surface primitive — the canonical card/panel container. Owns its
	// background/border/radius/padding from theme tokens. `tap` adds the
	// interactive hover/active affordance for tappable list items (e.g. session
	// rows); `as` lets it be a button/anchor when the whole surface is clickable.
	import type { Snippet } from 'svelte';

	let {
		tap = false,
		as = 'div',
		class: klass = '',
		children,
		...rest
	}: {
		tap?: boolean;
		as?: 'div' | 'button' | 'a' | 'li' | 'section' | 'form';
		class?: string;
		children?: Snippet;
		[key: string]: unknown;
	} = $props();
</script>

<svelte:element this={as} class="card {klass}" class:card-tap={tap} {...rest}>
	{@render children?.()}
</svelte:element>

<style>
	.card {
		background: var(--bg-elevated);
		border: 1px solid var(--border);
		border-radius: var(--r-lg);
		padding: var(--sp-4);
	}
	.card-tap {
		cursor: pointer;
		transition:
			border-color 0.12s var(--ease),
			background 0.12s var(--ease);
	}
	.card-tap:active {
		background: var(--bg-elevated-2);
	}
	.card-tap:hover {
		border-color: var(--border-strong);
	}
</style>
