<script lang="ts">
	// A compact count badge that sits before a parent session row (CCT-269) and
	// toggles its subagent group expanded/collapsed. Only rendered for groups with
	// >= 3 agents; smaller groups render inline, always expanded.
	let {
		count,
		running,
		open,
		label,
		ontoggle
	}: {
		count: number;
		running: number;
		open: boolean;
		// Tooltip context — e.g. "Workflow: deploy" or "subagents".
		label: string;
		ontoggle: () => void;
	} = $props();

	const done = $derived(count - running);
	const title = $derived(
		`${label} - ${count} total` +
			(running > 0 ? `, ${running} running` : '') +
			(done > 0 ? `, ${done} done` : '') +
			(open ? ' (click to collapse)' : ' (click to expand)')
	);
	const ariaLabel = $derived(`${open ? 'Collapse' : 'Expand'} ${title}`);
</script>

<button
	class="badge badge-info badge-toggle"
	class:open
	class:running={running > 0}
	type="button"
	{title}
	aria-label={ariaLabel}
	aria-expanded={open}
	onclick={(e) => {
		e.stopPropagation();
		ontoggle();
	}}
>
	{count}
</button>

<style>
	.badge-toggle {
		justify-content: center;
		min-width: 1.5rem;
		height: 1.5rem;
		padding: 0 var(--sp-2);
		border-radius: var(--r-pill);
		border-color: color-mix(in srgb, var(--info) 44%, transparent);
		background: color-mix(in srgb, var(--info) 13%, transparent);
		color: var(--info);
		font-size: var(--fs-xs);
		font-weight: var(--fw-semibold);
		line-height: 1;
		font-variant-numeric: tabular-nums;
		cursor: pointer;
	}
	.badge-toggle:hover {
		border-color: color-mix(in srgb, var(--info) 62%, transparent);
		background: color-mix(in srgb, var(--info) 20%, transparent);
	}
	.badge-toggle:focus-visible {
		outline: 2px solid var(--info);
		outline-offset: 2px;
	}
	.badge-toggle.running {
		border-color: color-mix(in srgb, var(--info) 68%, transparent);
		background: color-mix(in srgb, var(--info) 24%, transparent);
	}
	.badge-toggle.open {
		box-shadow: 0 0 0 2px color-mix(in srgb, var(--info) 18%, transparent);
	}
</style>
