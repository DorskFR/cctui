<script lang="ts">
	import { Link, Text, formatTimestamp } from '@dorsk/tsumikit';
	import { sessionHref } from '#lib/sessionFailureToast.ts';
	import { m } from '#lib/paraglide/messages.js';
	import type { FollowupMark } from './types';

	let {
		mark,
		onopen
	}: {
		mark: FollowupMark;
		/** Jump to the generation in place; the link still opens it in a new tab. */
		onopen?: (sessionId: string) => void;
	} = $props();

	const reasons = {
		cold: m.followup_thread_reason_cold,
		manual: m.followup_thread_reason_manual,
		auto: m.followup_thread_reason_auto,
		inbox: m.followup_thread_reason_inbox
	};
	const reason = $derived(mark.reason ? reasons[mark.reason]() : null);
	const to = $derived(mark.to);

	function open(e: MouseEvent) {
		if (!to || !onopen || e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
		e.preventDefault();
		onopen(to);
	}
</script>

<div
	class="followup-marker"
	role="separator"
	data-journey="followup-marker"
	data-followup-to={to ?? ''}
	aria-busy={mark.pending ? 'true' : undefined}
>
	<Text tone="faint" size="xs">
		{#if mark.pending || !to}
			{m.followup_thread_pending()}
		{:else}
			<Link href={sessionHref(to)} tone="muted" onclick={open}>{m.followup_thread_marker()}</Link>
		{/if}
		{' · '}{formatTimestamp(mark.at, 'datetime')}
		{#if reason}{' · '}{reason}{/if}
	</Text>
</div>

<style>
	.followup-marker {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		margin: var(--sp-2) 0;
		scroll-margin-top: var(--sp-3);
	}
	.followup-marker::before,
	.followup-marker::after {
		content: '';
		flex: 1;
		height: 1px;
		background: var(--border);
	}
</style>
