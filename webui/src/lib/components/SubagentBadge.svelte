<script lang="ts">
	// A compact count badge that sits at the START of a parent session's row
	// (CCT-269) and toggles its subagent group expanded/collapsed. Replaces the
	// old full-width subagent-group header row. Only rendered when a group has
	// >= 3 agents; smaller groups render inline (always expanded), no badge.
	//
	// The running/done breakdown is folded into the badge: the badge tints toward
	// the accent color while any agent is still running, and a tooltip spells out
	// the counts and (for workflow groups) the run label.
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
		`${label} — ${count} total` +
			(running > 0 ? `, ${running} running` : '') +
			(done > 0 ? `, ${done} done` : '') +
			(open ? ' (click to collapse)' : ' (click to expand)')
	);
</script>

<button
	class="badge-toggle"
	class:open
	class:active={running > 0}
	type="button"
	{title}
	onclick={(e) => {
		e.stopPropagation();
		ontoggle();
	}}
>
	<span class="caret" class:open>▸</span>
	<span class="n">{count}</span>
</button>

<style>
	.badge-toggle {
		flex: none;
		display: inline-flex;
		align-items: center;
		gap: 0.15rem;
		padding: 0.05rem var(--sp-2);
		border-radius: var(--r-sm);
		border: 1px solid var(--border-strong);
		background: var(--bg);
		color: var(--text-muted);
		font-size: var(--fs-xs);
		font-weight: var(--fw-semibold);
		line-height: 1.4;
		cursor: pointer;
	}
	.badge-toggle:hover {
		background: var(--bg-elevated);
	}
	.badge-toggle.active {
		border-color: color-mix(in srgb, var(--accent) 55%, transparent);
		color: var(--accent);
	}
	.badge-toggle.open {
		background: var(--bg-elevated);
	}
	.caret {
		display: inline-block;
		transition: transform 0.1s ease;
		opacity: 0.7;
		font-size: 0.75em;
	}
	.caret.open {
		transform: rotate(90deg);
	}
	.n {
		font-variant-numeric: tabular-nums;
	}
</style>
