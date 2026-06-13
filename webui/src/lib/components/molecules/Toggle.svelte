<script lang="ts">
	// Toggle chip — a muted pill/chip that lights up (tinted) when `pressed`.
	// The "on" tint is driven by the `--toggle-accent` CSS var (default accent),
	// so callers recolor it per-use (e.g. warm for behavior, role color for
	// message-type filters) without restyling the base. Used across the
	// conversation toolbar (filters / formatting / behavior / mobile tabs).
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';

	let {
		pressed = false,
		pill = false,
		struck = false,
		class: klass = '',
		children,
		...rest
	}: HTMLButtonAttributes & {
		pressed?: boolean;
		pill?: boolean;
		struck?: boolean;
		children?: Snippet;
	} = $props();
</script>

<button
	{...rest}
	type="button"
	class="toggle {klass}"
	class:pill
	class:struck
	class:on={pressed}
	aria-pressed={pressed}
>
	{@render children?.()}
</button>

<style>
	.toggle {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 0.15rem var(--sp-2);
		border-radius: var(--r-sm);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		line-height: 1.4;
		white-space: nowrap;
		background: var(--bg-elevated-2);
		color: var(--text-muted);
		border: 1px solid var(--border);
		cursor: pointer;
		transition:
			background 0.12s var(--ease),
			border-color 0.12s var(--ease),
			color 0.12s var(--ease);
	}
	.toggle.pill {
		border-radius: var(--r-pill);
	}
	.toggle:hover:not(:disabled) {
		border-color: var(--border-strong);
	}
	.toggle.on {
		--tc: var(--toggle-accent, var(--accent));
		color: var(--tc);
		border-color: color-mix(in srgb, var(--tc) 55%, transparent);
		background: color-mix(in srgb, var(--tc) 16%, transparent);
	}
	.toggle.struck {
		text-decoration: line-through;
	}
</style>
