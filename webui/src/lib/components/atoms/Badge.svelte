<script lang="ts">
	// Base badge/pill primitive. Owns the shape + tone palette from theme tokens.
	// Polymorphic via `as` so it can be a static <span> or an interactive
	// <button> (e.g. SubagentBadge). Specialized badges compose this and add only
	// their specifics (per-machine hue, toggle state).
	import type { Snippet } from 'svelte';

	type Tone = 'neutral' | 'ok' | 'warn' | 'danger' | 'info';

	let {
		tone = 'neutral',
		as = 'span',
		class: klass = '',
		children,
		...rest
	}: {
		tone?: Tone;
		as?: 'span' | 'button';
		class?: string;
		children?: Snippet;
		[key: string]: unknown;
	} = $props();
</script>

<svelte:element
	this={as}
	class="badge {klass}"
	class:badge-ok={tone === 'ok'}
	class:badge-warn={tone === 'warn'}
	class:badge-danger={tone === 'danger'}
	class:badge-info={tone === 'info'}
	{...rest}
>
	{@render children?.()}
</svelte:element>

<style>
	.badge {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		padding: 0.15rem var(--sp-2);
		border-radius: var(--r-pill);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		line-height: 1.4;
		background: var(--bg-elevated-2);
		color: var(--text-muted);
		border: 1px solid var(--border);
		white-space: nowrap;
	}
	.badge-ok {
		color: var(--ok);
		border-color: color-mix(in srgb, var(--ok) 40%, transparent);
		background: color-mix(in srgb, var(--ok) 12%, transparent);
	}
	.badge-warn {
		color: var(--warn);
		border-color: color-mix(in srgb, var(--warn) 40%, transparent);
		background: color-mix(in srgb, var(--warn) 12%, transparent);
	}
	.badge-danger {
		color: var(--danger);
		border-color: color-mix(in srgb, var(--danger) 40%, transparent);
		background: color-mix(in srgb, var(--danger) 12%, transparent);
	}
	.badge-info {
		color: var(--info);
		border-color: color-mix(in srgb, var(--info) 40%, transparent);
		background: color-mix(in srgb, var(--info) 12%, transparent);
	}
</style>
