<script lang="ts">
	import { Button, ConfirmModal, Input, KeyValue, SegmentedControl, Switch, Text } from '@dorsk/tsumikit';
	import { errMessage } from '$lib/api';
	import {
		actOnMessage,
		closeLink,
		expiryFor,
		listLinkMessages,
		updateLink,
		type ExpiryChoice,
		type LinkMessage,
		type LinkSettings,
		type LinkView,
		type MessageAction
	} from '$lib/cctuiverse';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	let { link, onchange }: { link: LinkView; onchange: (link: LinkView) => void } = $props();

	let busy = $state(false);
	let confirmClose = $state(false);
	let held = $state<LinkMessage[]>([]);
	let review = $state<LinkMessage[]>([]);
	let maxDraft = $state('');

	$effect(() => {
		maxDraft = link.settings.max_messages == null ? '' : String(link.settings.max_messages);
	});

	$effect(() => {
		const id = link.id;
		const wantHeld = link.held_count > 0;
		const wantReview = link.review_count > 0;
		void (async () => {
			try {
				held = wantHeld ? await listLinkMessages(id, 'held') : [];
				review = wantReview ? await listLinkMessages(id, 'review') : [];
			} catch (e) {
				toasts.error(errMessage(e));
			}
		})();
	});

	async function run<T>(fn: () => Promise<T>): Promise<T | undefined> {
		busy = true;
		try {
			return await fn();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}

	async function patch(p: Partial<LinkSettings>) {
		const next = await run(() => updateLink(link.id, p));
		if (next) onchange(next);
	}

	async function act(msg: LinkMessage, action: MessageAction) {
		await run(() => actOnMessage(link.id, msg.id, action));
		held = held.filter((x) => x.id !== msg.id);
		review = review.filter((x) => x.id !== msg.id);
	}

	function saveMax() {
		const t = maxDraft.trim();
		const n = t === '' ? null : Number.parseInt(t, 10);
		if (n !== null && (!Number.isFinite(n) || n < 0)) return;
		if (n === link.settings.max_messages) return;
		void patch({ max_messages: n });
	}

	const expiry = $derived<ExpiryChoice | ''>(link.settings.expires_at === null ? 'never' : '');
	const active = $derived(link.state === 'active');
</script>

<div class="panel">
	<KeyValue
		dense
		rows={[
			{ label: m.cctuiverse_peer(), value: link.peer_label ?? m.cctuiverse_invite_pending() },
			{ label: m.cctuiverse_peer_host(), value: link.peer_host ?? '—' },
			{ label: m.cctuiverse_safety_code_label(), value: link.safety_code ?? '—', mono: true },
			{
				label: m.cctuiverse_expires(),
				value: link.settings.expires_at
					? new Date(link.settings.expires_at).toLocaleString()
					: m.cctuiverse_expiry_never()
			},
			{ label: m.cctuiverse_sent(), value: String(link.sent_count) }
		]}
	/>

	{#if active}
		<Text size="xs" tone="muted">{m.cctuiverse_inbound()}</Text>
		<SegmentedControl
			size="sm"
			label={m.cctuiverse_inbound()}
			value={link.settings.inbound}
			options={[
				{ value: 'deliver', label: m.cctuiverse_inbound_deliver() },
				{ value: 'hold', label: m.cctuiverse_inbound_hold() }
			]}
			onchange={(v) => void patch({ inbound: v as LinkSettings['inbound'] })}
		/>
		<Text size="xs" tone="muted">{m.cctuiverse_outbound()}</Text>
		<SegmentedControl
			size="sm"
			label={m.cctuiverse_outbound()}
			value={link.settings.outbound}
			options={[
				{ value: 'tool', label: m.cctuiverse_outbound_tool() },
				{ value: 'auto', label: m.cctuiverse_outbound_auto() },
				{ value: 'both', label: m.cctuiverse_outbound_both() }
			]}
			onchange={(v) => void patch({ outbound: v as LinkSettings['outbound'] })}
		/>
		<Switch
			bind:checked={() => link.settings.review_outbound, (v) => void patch({ review_outbound: v })}
			label={m.cctuiverse_review_outbound()}
			disabled={busy}
		/>
		<Switch
			bind:checked={() => link.settings.share_transcript, (v) => void patch({ share_transcript: v })}
			label={m.cctuiverse_share_transcript()}
			disabled={busy}
		/>
		<Text size="xs" tone="muted">{m.cctuiverse_expiry()}</Text>
		<SegmentedControl
			size="sm"
			label={m.cctuiverse_expiry()}
			value={expiry}
			options={[
				{ value: '24h', label: m.cctuiverse_expiry_24h() },
				{ value: '7d', label: m.cctuiverse_expiry_7d() },
				{ value: 'never', label: m.cctuiverse_expiry_never() }
			]}
			onchange={(v) => void patch({ expires_at: expiryFor(v as ExpiryChoice, Date.now()) })}
		/>
		<Input
			bind:value={maxDraft}
			size="sm"
			inputmode="numeric"
			placeholder={m.cctuiverse_max_messages_placeholder()}
			aria-label={m.cctuiverse_max_messages()}
			disabled={busy}
			onsubmit={saveMax}
			onblur={saveMax}
		/>
	{/if}

	{#if held.length > 0}
		<Text size="xs" weight="semibold">{m.cctuiverse_held({ count: held.length })}</Text>
		{#each held as msg (msg.id)}
			<div class="msg">
				<Text size="xs" truncate>{msg.text}</Text>
				<Button size="sm" variant="ghost" disabled={busy} onclick={() => act(msg, 'release')}>
					{m.cctuiverse_release()}
				</Button>
				<Button size="sm" variant="ghost" disabled={busy} onclick={() => act(msg, 'drop')}>
					{m.cctuiverse_drop()}
				</Button>
			</div>
		{/each}
	{/if}

	{#if review.length > 0}
		<Text size="xs" weight="semibold">{m.cctuiverse_review({ count: review.length })}</Text>
		{#each review as msg (msg.id)}
			<div class="msg">
				<Text size="xs" truncate>{msg.text}</Text>
				<Button size="sm" variant="ghost" disabled={busy} onclick={() => act(msg, 'approve')}>
					{m.cctuiverse_approve()}
				</Button>
				<Button size="sm" variant="ghost" disabled={busy} onclick={() => act(msg, 'drop')}>
					{m.cctuiverse_drop()}
				</Button>
			</div>
		{/each}
	{/if}

	<Button size="sm" variant="danger" disabled={busy} onclick={() => (confirmClose = true)}>
		{link.state === 'pending' ? m.cctuiverse_revoke() : m.cctuiverse_close_link()}
	</Button>
</div>

{#if confirmClose}
	<ConfirmModal
		open
		tone="danger"
		title={m.cctuiverse_close_link()}
		message={m.cctuiverse_close_confirm({ peer: link.peer_label ?? m.cctuiverse_invite_pending() })}
		confirmLabel={m.cctuiverse_close_link()}
		onconfirm={async () => {
			confirmClose = false;
			const next = await run(() => closeLink(link.id));
			if (next) onchange(next);
		}}
		oncancel={() => (confirmClose = false)}
	/>
{/if}

<style>
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 16rem;
	}
	.msg {
		display: flex;
		gap: var(--sp-1);
		align-items: center;
		min-width: 0;
	}
</style>
