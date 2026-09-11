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
		type = null,
		ontoggle
	}: {
		count: number;
		running: number;
		open: boolean;
		// Tooltip context — e.g. "Workflow: deploy" or "subagents".
		label: string;
		// The group's agent type (`general-purpose`, `Explore`, …), shown
		// ahead of the count so a fan-out reads as what it is. Null for the
		// anonymous and workflow groups, which stay bare count chips.
		type?: string | null;
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

	const chip = '--badge-fs: var(--fs-sm); --badge-fw: var(--fw-semibold); --badge-min-size: 1.5rem;';
	// `.active` fills from `--badge-tone` and ignores these, so the open state still wins.
	const runningTint =
		' --badge-border: color-mix(in srgb, var(--info) 68%, transparent);' +
		' --badge-bg: color-mix(in srgb, var(--info) 24%, transparent);';
	const badgeStyle = $derived(running > 0 ? chip + runningTint : chip);
</script>

<span class="subagent-badge">
	<Badge
		as="button"
		tone="info"
		size="sm"
		numeric={!type}
		active={open}
		style={badgeStyle}
		{title}
		aria-label={ariaLabel}
		aria-expanded={open}
		onclick={(e: MouseEvent) => {
			e.stopPropagation();
			ontoggle();
		}}
	>
		{#if type}<span class="type">{type}</span>{/if}{count}
	</Badge>
</span>
