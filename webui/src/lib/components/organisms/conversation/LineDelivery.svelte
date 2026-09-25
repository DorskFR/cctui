<script lang="ts">
	// Delivery state of a user line in the meta row: failed (+ retry/edit),
	// sending, removed from queue, queued, or scheduled.
	import { Button, Icon, IconButton, Text } from '@dorsk/tsumikit';
	import type { Line } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		ln,
		archived,
		queueWaiting,
		onretry,
		onedit
	}: {
		ln: Line;
		archived: boolean;
		queueWaiting: boolean;
		onretry: (ts: number) => void;
		onedit: (text: string, ts: number) => void;
	} = $props();
</script>

{#if ln.failed}
	<span class="meta-end">
		<Text tone="danger" size="xs" nowrap title={ln.failed}>{m.conversation_not_delivered()}</Text>
	</span>
	{#if !archived}
		<Button
			variant="link"
			tone="danger"
			title={m.conversation_resend_title({ reason: ln.failed })}
			onclick={() => onretry(ln.ts)}>↻ {m.common_retry()}</Button>
		<IconButton
			inline
			icon="edit"
			label={m.conversation_edit_message_label()}
			title={m.conversation_edit_message_title()}
			onclick={() => onedit(ln.text ?? '', ln.ts)}
		/>
	{/if}
{:else if ln.pending}
	<span class="meta-end">
		{#if ln.retrying}
			<Text tone="warn" size="xs" title={m.conversation_retrying_title()}
				>{m.conversation_retrying({ attempt: ln.retrying.attempt, max: ln.retrying.max })}</Text
			>
		{:else}
			<Text tone="warn" size="xs">{m.conversation_sending()}</Text>
		{/if}
	</span>
	{#if !archived}
		<IconButton
			inline
			icon="edit"
			label={m.conversation_edit_pending_label()}
			title={m.conversation_edit_pending_title()}
			onclick={() => onedit(ln.text ?? '', ln.ts)}
		/>
	{/if}
{:else if ln.cancelled}
	<span class="meta-end">
		<Text tone="faint" size="xs" nowrap>{m.conversation_queue_removed()}</Text>
	</span>
{:else if queueWaiting}
	<span class="meta-end">
		<Text tone="faint" size="xs" nowrap>{m.conversation_queued()}</Text>
	</span>
{:else if ln.scheduledAt !== undefined}
	{@const time = new Date(ln.scheduledAt).toLocaleTimeString([], {
		hour: '2-digit',
		minute: '2-digit'
	})}
	<span
		class="meta-end scheduled-mark"
		title={m.conversation_scheduled_for({ time })}
		aria-label={m.conversation_scheduled_for({ time })}
	>
		<Icon name="clock" size={12} />
	</span>
{/if}

<style>
	/* Pushes the send-status text (and the controls after it) to the right. */
	.meta-end {
		margin-left: auto;
	}
	.scheduled-mark {
		color: var(--text-faint);
		display: inline-flex;
	}
</style>
