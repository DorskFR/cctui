<script lang="ts">
	// Toggle chip — a muted pill/chip that lights up (tinted) when `pressed`.
	// The "on" tint is driven by the `--toggle-accent` CSS var (default accent),
	// so callers recolor it per-use (e.g. warm for behavior, role color for
	// message-type filters) without restyling the base. Used across the
	// conversation toolbar (filters / formatting / behavior / mobile tabs).
	//
	// Specializes the Button atom (ghost variant) for shared disabled/focus/
	// transition behaviour; the chip surface + "on" tint are overrides.
	// `.btn.toggle` outranks the atom's :where()-scoped variant classes.
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import Button from '$lib/components/atoms/Button.svelte';

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

<Button
	{...rest}
	variant="ghost"
	class="toggle {pill ? 'pill' : ''} {struck ? 'struck' : ''} {pressed ? 'on' : ''} {klass}"
	aria-pressed={pressed}
>
	{@render children?.()}
</Button>

<style>
	:global(.btn.toggle) {
		gap: 4px;
		min-height: 0;
		padding: 0.15rem var(--sp-2);
		border-radius: var(--r-sm);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		line-height: 1.4;
		background: var(--bg-elevated-2);
		color: var(--text-muted);
		border: 1px solid var(--border);
	}
	:global(.btn.toggle.pill) {
		border-radius: var(--r-pill);
	}
	:global(.btn.toggle:hover:not(:disabled)) {
		border-color: var(--border-strong);
		background: var(--bg-elevated-2);
	}
	:global(.btn.toggle.on) {
		--tc: var(--toggle-accent, var(--accent));
		color: var(--tc);
		border-color: color-mix(in srgb, var(--tc) 55%, transparent);
		background: color-mix(in srgb, var(--tc) 16%, transparent);
	}
	:global(.btn.toggle.struck) {
		text-decoration: line-through;
	}
</style>
