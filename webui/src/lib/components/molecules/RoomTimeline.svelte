<script lang="ts">
	import { Badge, EmptyState, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { RoomMessage } from '$lib/rooms';

	let {
		messages
	}: {
		/** Oldest first, as the timeline endpoint returns them. */
		messages: RoomMessage[];
	} = $props();
</script>

{#if messages.length === 0}
	<EmptyState title={m.rooms_empty()} />
{:else}
	<ol class="timeline">
		{#each messages as msg (msg.seq)}
			<li class="post" class:human={msg.sender_session_id === null}>
				<div class="head">
					{#if msg.sender_session_id === null}
						<Badge size="xs" uppercase>{m.rooms_from_human()}</Badge>
					{/if}
					<span class="from" title={msg.sender_label}>
						<Text size="xs" tone="muted">{msg.sender_label}</Text>
					</span>
					<Timestamp value={msg.created_at} mode="time" tone="faint" size="xs" />
				</div>
				<div class="body"><Text size="sm">{msg.body}</Text></div>
			</li>
		{/each}
	</ol>
{/if}

<style>
	.timeline {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.post {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
		padding: var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-md);
	}
	/* The human's own posts read as the near side of the conversation. */
	.human {
		border-color: var(--border-strong);
		background: var(--bg-elevated);
	}
	.head {
		display: flex;
		gap: var(--sp-2);
		align-items: center;
		min-width: 0;
	}
	/* A post is plain text, so its own line breaks have to survive. */
	.body {
		min-width: 0;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
	.from {
		min-width: 0;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
</style>
