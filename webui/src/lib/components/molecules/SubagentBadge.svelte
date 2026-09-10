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
		numeric
		active={open}
		class={`badge-toggle${running > 0 ? ' running' : ''}`}
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
	/* TSU gap: Badge has no prop for a count chip that reads a step larger and
	   bolder than its tone/size scale. */
	.subagent-badge :global(.badge-toggle) {
		min-width: 1.5rem;
		height: 1.5rem;
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
	}
	/* TSU gap: no Badge emphasis step between the idle tint and the `active` fill. */
	.subagent-badge :global(.badge-toggle.running:not(.active)) {
		border-color: color-mix(in srgb, var(--info) 68%, transparent);
		background: color-mix(in srgb, var(--info) 24%, transparent);
	}
</style>
