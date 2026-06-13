<script lang="ts">
	// Small inline label primitive — a flat, rounded tag for secondary metadata
	// (status, model/effort, cwd). Lighter than a Badge: no tone palette, just the
	// muted pill. Polymorphic via `as`; an `as="button"` chip is interactive
	// (hover affordance). `mono` switches to the monospace family (paths, ids).
	import type { Snippet } from 'svelte';

	let {
		as = 'span',
		mono = false,
		class: klass = '',
		children,
		...rest
	}: {
		as?: 'span' | 'button';
		mono?: boolean;
		class?: string;
		children?: Snippet;
		[key: string]: unknown;
	} = $props();
</script>

<svelte:element this={as} class="chip {klass}" class:mono class:interactive={as === 'button'} {...rest}>
	{@render children?.()}
</svelte:element>

<style>
	.chip {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		font-size: var(--fs-xs);
		color: var(--text-muted);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border);
		border-radius: var(--r-pill);
		padding: 0.1rem var(--sp-2);
		max-width: 100%;
	}
	.mono {
		font-family: var(--font-mono);
	}
	.interactive {
		cursor: pointer;
		transition:
			border-color 0.12s var(--ease),
			color 0.12s var(--ease);
	}
	.interactive:hover {
		border-color: var(--border-strong);
		color: var(--text);
	}
</style>
