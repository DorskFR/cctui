<script lang="ts">
	// Selection-card option — a bordered card that gains a colored ring + tint
	// when `selected`. The accent is driven by `--opt-accent` (default accent),
	// so each picker recolors per use (blue for run-target, brand color for the
	// adapter, green/blue/red for permission mode). `row` lays the content out
	// horizontally (icon + label) instead of the default label-over-hint column.
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';

	let {
		selected = false,
		row = false,
		class: klass = '',
		children,
		...rest
	}: HTMLButtonAttributes & {
		selected?: boolean;
		row?: boolean;
		children?: Snippet;
	} = $props();
</script>

<button
	{...rest}
	type="button"
	class="opt-btn {klass}"
	class:row
	class:sel={selected}
	aria-pressed={selected}
>
	{@render children?.()}
</button>

<style>
	.opt-btn {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: var(--sp-2);
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		color: var(--text);
		text-align: left;
		cursor: pointer;
		transition:
			background 0.12s var(--ease),
			border-color 0.12s var(--ease),
			color 0.12s var(--ease);
	}
	.opt-btn.row {
		flex-direction: row;
		align-items: center;
		justify-content: center;
		gap: var(--sp-2);
		color: var(--text-muted);
		font-weight: var(--fw-medium);
	}
	.opt-btn.sel {
		--oc: var(--opt-accent, var(--accent));
		border-color: var(--oc);
		background: color-mix(in srgb, var(--oc) 14%, var(--bg));
		color: var(--oc);
	}
	/* The slotted hint text (a global `.faint`) tints toward the accent too. */
	.opt-btn.sel :global(.faint) {
		color: color-mix(in srgb, var(--oc) 70%, var(--text-muted));
	}
</style>
