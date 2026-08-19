<script lang="ts">
	import { m } from '$lib/paraglide/messages';
	import type { TurnSummary } from './types';

	let { summary }: { summary: TurnSummary } = $props();
</script>

<div class="turn-summary" class:needs-action={summary.needsAction}>
	<span class="ts-label"
		>{summary.needsAction
			? m.conversation_turn_summary_needs_action()
			: m.conversation_turn_summary_label()}</span
	>
	<span class="ts-detail">{summary.detail}</span>
</div>

<style>
	/* A duller echo of the assistant bubble: same left rule, no fill, faint ink —
	   it must read as subtext hanging off the message, not as its own message. */
	.turn-summary {
		display: flex;
		align-items: baseline;
		flex-wrap: wrap;
		gap: var(--sp-2);
		margin-top: 2px;
		padding: var(--sp-1) var(--sp-3);
		border-left: 2px solid color-mix(in srgb, var(--role-summary) 40%, transparent);
		border-radius: 0 var(--r-sm) var(--r-sm) 0;
		background: color-mix(in srgb, var(--role-summary) 7%, transparent);
		color: var(--text-faint);
		font-size: var(--fs-xs);
		line-height: var(--lh-normal);
	}
	.ts-label {
		flex: none;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		font-weight: var(--fw-semibold);
	}
	.ts-detail {
		overflow-wrap: anywhere;
	}
	.turn-summary.needs-action {
		border-left-color: color-mix(in srgb, var(--warn) 65%, transparent);
		background: color-mix(in srgb, var(--warn) 10%, transparent);
		color: var(--warn);
	}
</style>
