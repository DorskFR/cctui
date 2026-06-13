<script lang="ts">
	// Heading primitive: the ONLY place an <h1>–<h6> is emitted. `level` picks both
	// the semantic tag and the default display size; `size` overrides the visual
	// size independently of the level (e.g. toolbar chrome pinned smaller than its
	// heading rank). All sizes/weights/colours come from theme tokens.
	import type { Snippet } from 'svelte';

	type Size = 'xs' | 'sm' | 'md' | 'lg' | 'xl' | '2xl';

	let {
		level = 2,
		size,
		tone = 'default',
		class: klass = '',
		children,
		...rest
	}: {
		level?: 1 | 2 | 3 | 4 | 5 | 6;
		size?: Size;
		tone?: 'default' | 'muted' | 'faint';
		class?: string;
		children?: Snippet;
		[key: string]: unknown;
	} = $props();

	// Default display size per rank (h1 biggest). `size` overrides it.
	const DEFAULT_SIZE: Record<number, Size> = { 1: '2xl', 2: 'xl', 3: 'lg', 4: 'md', 5: 'sm', 6: 'xs' };
	const fs = $derived(size ?? DEFAULT_SIZE[level]);
</script>

<svelte:element this={`h${level}`} class="heading fs-{fs} tone-{tone} {klass}" {...rest}>
	{@render children?.()}
</svelte:element>

<style>
	.heading {
		margin: 0;
		font-weight: var(--fw-semibold);
		line-height: var(--lh-tight);
		color: var(--text);
	}
	.tone-muted {
		color: var(--text-muted);
	}
	.tone-faint {
		color: var(--text-faint);
	}
	.fs-xs {
		font-size: var(--fs-xs);
	}
	.fs-sm {
		font-size: var(--fs-sm);
	}
	.fs-md {
		font-size: var(--fs-md);
	}
	.fs-lg {
		font-size: var(--fs-lg);
	}
	.fs-xl {
		font-size: var(--fs-xl);
	}
	.fs-2xl {
		font-size: var(--fs-2xl);
	}
</style>
