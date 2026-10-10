<script lang="ts">
	import { Button, Text, Timestamp } from '@dorsk/tsumikit';
	import type { LinkMessage, MessageAction } from '$lib/cctuiverse';

	let {
		msg,
		actions,
		busy,
		error,
		onact
	}: {
		msg: LinkMessage;
		actions: { action: MessageAction; label: string }[];
		busy: boolean;
		error: string | null;
		onact: (action: MessageAction) => void;
	} = $props();
</script>

<div class="item">
	<Timestamp value={msg.created_at} mode="time" tone="faint" size="xs" />
	<pre class="text">{msg.text}</pre>
	{#if error}
		<Text size="xs" tone="danger">{error}</Text>
	{/if}
	<div class="acts">
		{#each actions as a (a.action)}
			<Button size="sm" variant="ghost" disabled={busy} onclick={() => onact(a.action)}>
				{a.label}
			</Button>
		{/each}
	</div>
</div>

<style>
	.item {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		padding: var(--sp-1);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
	}
	.text {
		margin: 0;
		max-height: 12rem;
		overflow: auto;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		font-size: var(--fs-xs);
	}
	.acts {
		display: flex;
		gap: var(--sp-1);
		justify-content: flex-end;
	}
</style>
