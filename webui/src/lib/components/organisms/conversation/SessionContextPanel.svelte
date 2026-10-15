<script lang="ts">
	import { Badge, Button, Text } from '@dorsk/tsumikit';
	import ContextPicker from '$lib/components/molecules/ContextPicker.svelte';
	import { useAttachContext, useContextItems } from '$lib/queries';
	import type { SessionContextResponse } from '@bindings/SessionContextResponse';
	import { errMessage } from '$lib/api';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		sessionId,
		attached,
		canAttach
	}: {
		sessionId: string;
		attached: SessionContextResponse | undefined;
		canAttach: boolean;
	} = $props();

	const items = useContextItems(() => canAttach);
	const attach = useAttachContext();
	const memories = $derived((items.data ?? []).filter((i) => i.enabled && i.kind === 'memory'));
	let picks = $state<string[]>([]);
	let busy = $state(false);

	async function submit() {
		if (!picks.length || busy) return;
		busy = true;
		try {
			const res = await attach(sessionId, picks);
			if (res.attached.length)
				toasts.ok(m.conversation_context_attached({ names: res.attached.join(', ') }));
			if (res.unknown.length)
				toasts.error(m.conversation_context_unknown({ names: res.unknown.join(', ') }));
			picks = [];
		} catch (e) {
			toasts.error(m.conversation_context_attach_failed({ error: errMessage(e) }));
		} finally {
			busy = false;
		}
	}
</script>

<div class="panel">
	{#each [{ title: m.conversation_context_rules(), rows: attached?.rules ?? [] }, { title: m.conversation_context_memories(), rows: attached?.memories ?? [] }] as group (group.title)}
		<div class="group">
			<Text size="xs" weight="medium">{group.title}</Text>
			{#if group.rows.length}
				<ul>
					{#each group.rows as r (r.id)}
						<li>
							<span class="title">{r.title}</span>
							<Badge size="xs" tone="muted">v{r.version}</Badge>
						</li>
					{/each}
				</ul>
			{:else}
				<Text size="xs" tone="faint">{m.conversation_context_none()}</Text>
			{/if}
		</div>
	{/each}
	{#if canAttach}
		<div class="group">
			<Text size="xs" weight="medium">{m.conversation_context_attach()}</Text>
			<ContextPicker items={memories} bind:value={picks} label={m.conversation_context_attach()} />
			<Button size="sm" variant="primary" disabled={busy || !picks.length} onclick={submit}
				>{m.conversation_context_attach_btn({ count: picks.length })}</Button
			>
		</div>
	{/if}
</div>

<style>
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		padding: var(--sp-2);
		width: min(22rem, 90vw);
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	ul {
		margin: 0;
		padding: 0;
		list-style: none;
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	li {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		font-size: var(--fs-sm);
	}
	.title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
