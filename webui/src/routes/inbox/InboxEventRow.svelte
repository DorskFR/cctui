<script lang="ts">
	import { Badge, Button, Link, Text, Timestamp } from '@dorsk/tsumikit';
	import { errMessage } from '#lib/api.ts';
	import type { InboxEvent } from '#lib/queries/inbox.ts';
	import { sessionHref } from '#lib/sessionFailureToast.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	let {
		event,
		oncancel,
		onretry
	}: {
		event: InboxEvent;
		oncancel: (id: string) => Promise<void>;
		onretry: (id: string) => Promise<void>;
	} = $props();

	let busy = $state(false);
	const landed = $derived(event.delivered_session_id);
	const waiting = $derived(
		event.status === 'pending' && Date.parse(event.not_before) > Date.now() ? event.not_before : null
	);

	async function run(action: (id: string) => Promise<void>) {
		busy = true;
		try {
			await action(event.id);
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}
</script>

<div class="row" data-journey="inbox-event" data-status={event.status}>
	<div class="head">
		<Badge size="xs" uppercase>{event.source}</Badge>
		<Timestamp value={Date.parse(event.created_at)} mode="relative" tone="faint" size="xs" />
		{#if waiting}
			<Text size="xs" tone="faint">{m.inbox_waiting_until({ at: new Date(waiting).toLocaleString() })}</Text>
		{/if}
		{#if event.attempts > 0}
			<Text size="xs" tone="faint">{m.inbox_attempts({ count: event.attempts })}</Text>
		{/if}
		{#if landed}
			<Text size="xs" tone="muted">
				{m.inbox_landed_in()}
				<Link href={sessionHref(landed)}>{landed.slice(0, 8)}</Link>
				{#if event.delivered_as}{m.inbox_delivered_as({ mode: event.delivered_as })}{/if}
			</Text>
		{/if}
		<span class="actions">
			{#if event.status === 'pending'}
				<Button size="sm" variant="link" disabled={busy} onclick={() => run(oncancel)}>
					{m.inbox_cancel()}
				</Button>
			{:else if event.status === 'dead'}
				<Button size="sm" variant="link" disabled={busy} onclick={() => run(onretry)}>
					{m.inbox_retry()}
				</Button>
			{/if}
		</span>
	</div>
	<Text size="sm" truncate>{event.body}</Text>
	{#if event.last_error}
		<Text size="xs" tone="danger">{event.last_error}</Text>
	{/if}
</div>

<style>
	.row {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		padding: var(--sp-2) 0;
		border-top: 1px solid var(--border);
	}
	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}
	.actions {
		margin-left: auto;
	}
</style>
