<script lang="ts">
	import type { Webhook } from '@bindings/Webhook';
	import type { WebhookSecret } from '@bindings/WebhookSecret';
	import { Button, ConfirmModal, Spinner, Text } from '@dorsk/tsumikit';
	import PageHead from '#lib/components/molecules/PageHead.svelte';
	import WebhookEditorModal from '#lib/components/organisms/webhooks/WebhookEditorModal.svelte';
	import WebhookSecretModal from '#lib/components/organisms/webhooks/WebhookSecretModal.svelte';
	import { errMessage } from '#lib/api.ts';
	import { usePresets } from '#lib/queries/presets.ts';
	import { useWebhookActions, useWebhooks } from '#lib/queries/webhooks.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { withEnabled } from '#lib/webhooks/webhookForm.ts';
	import WebhookRow from './WebhookRow.svelte';

	const hooks = useWebhooks();
	const presets = usePresets();
	const actions = useWebhookActions();
	const presetNames = $derived(new Map((presets.data ?? []).map((p) => [p.id, p.name])));

	let editor = $state<{ hook: Webhook | null } | null>(null);
	let issued = $state<WebhookSecret | null>(null);
	let deleting = $state<Webhook | null>(null);
	let rotating = $state<Webhook | null>(null);
	let busyId = $state<string | null>(null);

	async function act<T>(hook: Webhook, work: () => Promise<T>): Promise<T | undefined> {
		busyId = hook.id;
		try {
			return await work();
		} catch (e) {
			toasts.error(errMessage(e));
			return undefined;
		} finally {
			busyId = null;
		}
	}

	const toggle = (hook: Webhook, enabled: boolean) =>
		act(hook, () => actions.update(hook.id, withEnabled(hook, enabled)));
	async function remove() {
		const hook = deleting;
		if (!hook) return;
		await act(hook, () => actions.remove(hook.id));
		deleting = null;
	}
	async function rotate() {
		const hook = rotating;
		if (!hook) return;
		const fresh = await act(hook, () => actions.rotate(hook.id));
		rotating = null;
		if (fresh) issued = fresh;
	}
</script>

<div class="page" data-journey="webhooks">
	<PageHead title={m.webhooks_title()}>
		<Button variant="primary" onclick={() => (editor = { hook: null })}>{m.webhooks_new()}</Button>
	</PageHead>
	<Text size="sm" tone="faint">{m.webhooks_subtitle()}</Text>

	{#if hooks.isLoading}
		<div class="msg"><Spinner /></div>
	{:else if hooks.error}
		<div class="msg"><Text tone="danger">{errMessage(hooks.error)}</Text></div>
	{:else if (hooks.data ?? []).length === 0}
		<div class="msg"><Text tone="faint">{m.webhooks_empty()}</Text></div>
	{:else}
		<div class="list">
			{#each hooks.data ?? [] as hook (hook.id)}
				<WebhookRow
					{hook}
					presetName={presetNames.get(hook.preset_id) ?? null}
					busy={busyId === hook.id}
					ontoggle={(enabled) => toggle(hook, enabled)}
					onedit={() => (editor = { hook })}
					onrotate={() => (rotating = hook)}
					ondelete={() => (deleting = hook)}
				/>
			{/each}
		</div>
	{/if}
</div>

{#if editor}
	<WebhookEditorModal
		hook={editor.hook}
		presets={presets.data ?? []}
		onclose={() => (editor = null)}
		oncreated={(created) => (issued = created)}
	/>
{/if}

{#if issued}
	<WebhookSecretModal {issued} onclose={() => (issued = null)} />
{/if}

<ConfirmModal
	bind:open={() => rotating !== null, (v) => { if (!v) rotating = null; }}
	tone="warn"
	title={m.webhooks_rotate_title()}
	message={m.webhooks_rotate_message({ name: rotating?.name ?? '' })}
	confirmLabel={m.webhooks_rotate()}
	cancelLabel={m.common_cancel()}
	busy={busyId !== null && busyId === rotating?.id}
	onconfirm={rotate}
/>

<ConfirmModal
	bind:open={() => deleting !== null, (v) => { if (!v) deleting = null; }}
	tone="danger"
	title={m.webhooks_delete_title()}
	message={m.webhooks_delete_message({ name: deleting?.name ?? '' })}
	confirmLabel={m.common_delete()}
	cancelLabel={m.common_cancel()}
	busy={busyId !== null && busyId === deleting?.id}
	onconfirm={remove}
/>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		padding: var(--sp-4);
		max-width: 60rem;
		width: 100%;
		margin: 0 auto;
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.msg {
		display: grid;
		place-items: center;
		padding: var(--sp-6);
	}
</style>
