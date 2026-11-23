<script lang="ts">
	// A compact count badge that sits before a parent session row and
	// toggles its subagent group expanded/collapsed. Only rendered for groups with
	// >= 3 agents; smaller groups render inline, always expanded. Composes the
	// base Badge as an interactive (info-toned) button.
	import { Badge } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';

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

	const chip =
		'font-size: var(--fs-sm); font-weight: var(--fw-semibold); min-width: 1.85em; min-height: 1.85em; position: relative;';
	// Inline colours would beat the `.active` fill, so the open state carries none.
	const runningTint =
		' border-color: color-mix(in srgb, var(--info) 68%, transparent);' +
		' background: color-mix(in srgb, var(--info) 24%, transparent);';
	const badgeStyle = $derived(running > 0 && !open ? chip + runningTint : chip);
</script>

<span class="subagent-badge">
	<Badge
		as="button"
		tone="info"
		size="sm"
		numeric
		active={open}
		style={badgeStyle}
		{title}
		aria-label={ariaLabel}
		aria-expanded={open}
		onpointerdown={(e: PointerEvent) => {
			e.stopPropagation();
			// The card's press scale shifts the badge under a stationary pointer.
			(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		}}
		onclick={(e: MouseEvent) => {
			e.stopPropagation();
			ontoggle();
		}}
	>
		<span class="slop" aria-hidden="true"></span>{count}
	</Badge>
</span>

<style>
	/* Transparent hit slop so a near-miss toggles the group instead of opening the session. */
	.slop {
		position: absolute;
		inset: calc(-1 * var(--sp-1));
		border-radius: inherit;
	}
</style>
