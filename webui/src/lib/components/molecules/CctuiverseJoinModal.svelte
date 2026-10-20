<script lang="ts">
	import { Button, Callout, Field, Input, Modal, Text, Textarea } from '@dorsk/tsumikit';
	import { errMessage } from '$lib/api';
	import { findInvite, joinInvite, type LinkView } from '$lib/cctuiverse';
	import { m } from '$lib/paraglide/messages';

	let {
		sessionId,
		defaultLabel,
		onclose
	}: { sessionId: string; defaultLabel: string; onclose: () => void } = $props();

	let pasted = $state('');
	// svelte-ignore state_referenced_locally
	let label = $state(defaultLabel);
	let busy = $state(false);
	let error = $state<string | null>(null);
	let link = $state<LinkView | null>(null);

	const invite = $derived(findInvite(pasted));

	async function join() {
		if (!invite || !label.trim()) return;
		busy = true;
		error = null;
		try {
			link = await joinInvite(invite, sessionId, label.trim());
		} catch (e) {
			error = errMessage(e);
		} finally {
			busy = false;
		}
	}
</script>

<Modal title={m.cctuiverse_join_title()} {onclose} {busy}>
	{#snippet body()}
		<div class="col">
			{#if link}
				<Callout tone="success" title={m.cctuiverse_linked_with({ peer: link.peer_label ?? '' })}>
					{m.cctuiverse_safety_code({ code: link.safety_code ?? '—' })}
				</Callout>
				<Text size="xs" tone="muted">{m.cctuiverse_safety_code_hint()}</Text>
			{:else}
				<Field label={m.cctuiverse_join_paste()}>
					<Textarea bind:value={pasted} rows={3} disabled={busy} />
				</Field>
				{#if pasted.trim() && !invite}
					<Callout tone="warn">{m.cctuiverse_join_not_invite()}</Callout>
				{/if}
				<Field label={m.cctuiverse_label_label()} hint={m.cctuiverse_label_hint()}>
					<Input bind:value={label} maxlength={80} disabled={busy} />
				</Field>
			{/if}
			{#if error}
				<Callout tone="danger">{error}</Callout>
			{/if}
		</div>
	{/snippet}
	{#snippet footer()}
		{#if link}
			<Button size="sm" variant="ghost" onclick={onclose}>{m.common_close()}</Button>
		{:else}
			<Button size="sm" variant="ghost" onclick={onclose}>{m.common_cancel()}</Button>
			<Button size="sm" variant="primary" disabled={busy || !invite || !label.trim()} onclick={join}>
				{m.cctuiverse_join_action()}
			</Button>
		{/if}
	{/snippet}
</Modal>

<style>
	.col {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
</style>
