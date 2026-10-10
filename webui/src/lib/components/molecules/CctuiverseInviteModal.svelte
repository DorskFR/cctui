<script lang="ts">
	import { Button, Callout, CopyButton, Field, Input, Modal, Spinner, Text } from '@dorsk/tsumikit';
	import { errMessage } from '$lib/api';
	import { now } from '$lib/clock.svelte';
	import {
		createInvite,
		formatCountdown,
		listLinks,
		remainingMs,
		type InviteTarget,
		type LinkView
	} from '$lib/cctuiverse';
	import { ws } from '$lib/ws.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		target,
		defaultLabel,
		onclose
	}: { target: InviteTarget; defaultLabel: string; onclose: () => void } = $props();

	// svelte-ignore state_referenced_locally
	let label = $state(defaultLabel);
	let busy = $state(false);
	let error = $state<string | null>(null);
	let link = $state<LinkView | null>(null);
	let invite = $state('');

	const left = $derived(link ? remainingMs(link.invite_expires_at, now(1000)) : null);

	async function create() {
		const l = label.trim();
		if (!l) return;
		busy = true;
		error = null;
		try {
			const res = await createInvite(target, l);
			link = res.link;
			invite = res.invite;
		} catch (e) {
			error = errMessage(e);
		} finally {
			busy = false;
		}
	}

	async function refresh() {
		const id = link?.id;
		if (!id) return;
		try {
			link = (await listLinks(target)).find((l) => l.id === id) ?? link;
		} catch {
			// The next change event retries.
		}
	}

	$effect(() => ws.onCctuiverseChanged(() => void refresh()));
</script>

<Modal title={m.cctuiverse_invite_title()} {onclose} {busy}>
	{#snippet body()}
		<div class="col">
			{#if !link}
				<Text size="sm" tone="muted">{m.cctuiverse_invite_intro()}</Text>
				<Field label={m.cctuiverse_label_label()} hint={m.cctuiverse_label_hint()}>
					<Input bind:value={label} maxlength={80} disabled={busy} onsubmit={create} />
				</Field>
			{:else if link.state === 'pending'}
				<Text size="sm">{m.cctuiverse_invite_share()}</Text>
				<div class="url">
					<Input value={invite} readonly mono grow aria-label={m.cctuiverse_invite_url()} />
					<CopyButton text={invite} label={m.common_copy()} />
				</div>
				{#if left !== null}
					<Text size="xs" tone="muted">
						{left > 0
							? m.cctuiverse_invite_expires_in({ time: formatCountdown(left) })
							: m.cctuiverse_invite_expired()}
					</Text>
				{/if}
				<div class="wait">
					<Spinner />
					<Text size="sm" tone="muted">{m.cctuiverse_invite_waiting()}</Text>
				</div>
			{:else if link.state === 'active'}
				<Callout tone="success" title={m.cctuiverse_linked_with({ peer: link.peer_label ?? '' })}>
					{m.cctuiverse_safety_code({ code: link.safety_code ?? '—' })}
				</Callout>
				<Text size="xs" tone="muted">{m.cctuiverse_safety_code_hint()}</Text>
			{:else}
				<Callout tone="warn">{m.cctuiverse_invite_closed()}</Callout>
			{/if}
			{#if error}
				<Callout tone="danger">{error}</Callout>
			{/if}
		</div>
	{/snippet}
	{#snippet footer()}
		{#if !link}
			<Button size="sm" variant="ghost" onclick={onclose}>{m.common_cancel()}</Button>
			<Button size="sm" variant="primary" disabled={busy || !label.trim()} onclick={create}>
				{m.cctuiverse_invite_create()}
			</Button>
		{:else}
			<Button size="sm" variant="ghost" onclick={onclose}>{m.common_close()}</Button>
		{/if}
	{/snippet}
</Modal>

<style>
	.col {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.url {
		display: flex;
		gap: var(--sp-1);
		align-items: center;
	}
	.wait {
		display: flex;
		gap: var(--sp-2);
		align-items: center;
	}
</style>
