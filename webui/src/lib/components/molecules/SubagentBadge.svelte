<script lang="ts">
	// A compact count badge that sits before a parent session row and
	// toggles its subagent group expanded/collapsed. Only rendered for groups with
	// >= 3 agents; smaller groups render inline, always expanded. Composes the
	// base Badge as an interactive (info-toned) button.
	import { Badge } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

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
		m.sessions_subagent_total({ label, count }) +
			(running > 0 ? m.sessions_subagent_running({ count: running }) : '') +
			(done > 0 ? m.sessions_subagent_done({ count: done }) : '') +
			(open ? m.sessions_subagent_collapse() : m.sessions_subagent_expand())
	);
	const ariaLabel = $derived(`${open ? m.sessions_collapse() : m.sessions_expand()} ${title}`);
</script>

<span class="subagent-badge">
	<Badge
		as="button"
		tone="info"
		size="sm"
		active={open}
		class={`badge-toggle${running > 0 ? ' running' : ''}`}
		type="button"
		{title}
		aria-label={ariaLabel}
		aria-expanded={open}
		onclick={(e: MouseEvent) => {
			e.stopPropagation();
			ontoggle();
		}}
	>
		{count}
	</Badge>
</span>

<style>
	/* Colour/fill/hover/pill are tsumikit Badge's: tone="info" tints it, `active`
	   (open) fills it, size="sm" is the compact form. Only the count-chip sizing,
	   tabular digits, focus ring, and the running emphasis are reached in here —
	   scoped under the wrapper so the selectors can't leak. */
	.subagent-badge :global(.badge-toggle) {
		justify-content: center;
		min-width: 1.5rem;
		height: 1.5rem;
		/* Resolve the digit size through a --fs-* token so the global
		   font-scale picker grows the counter in step with surrounding session
		   text. min-width/height stay rem-pinned chrome, so the chip keeps its
		   compact footprint while only the glyph scales; tsumikit's size="sm"
		   font-size (not a --fs-* token) is what left it frozen before. */
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
		font-variant-numeric: tabular-nums;
	}
	.subagent-badge :global(.badge-toggle):focus-visible {
		outline: 2px solid var(--info);
		outline-offset: 2px;
	}
	/* Running (and not expanded): a deeper tint than the idle pill, short of the
	   full active fill. */
	.subagent-badge :global(.badge-toggle.running:not(.active)) {
		border-color: color-mix(in srgb, var(--info) 68%, transparent);
		background: color-mix(in srgb, var(--info) 24%, transparent);
	}
</style>
