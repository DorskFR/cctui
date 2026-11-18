<script lang="ts">
	import type { Webhook } from '@bindings/Webhook';
	import { Button, Card, CopyButton, Disclosure, IconButton, Switch, Text } from '@dorsk/tsumikit';
	import { apiBase } from '#lib/config.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { deliveryUrl } from '#lib/webhooks/webhookForm.ts';
	import WebhookDeliveries from './WebhookDeliveries.svelte';

	let {
		hook,
		presetName,
		busy = false,
		ontoggle,
		onedit,
		onrotate,
		ondelete
	}: {
		hook: Webhook;
		presetName: string | null;
		busy?: boolean;
		ontoggle: (enabled: boolean) => void;
		onedit: () => void;
		onrotate: () => void;
		ondelete: () => void;
	} = $props();

	let logOpen = $state(false);
	const url = $derived(deliveryUrl(apiBase(), hook.id));
</script>

<Card padding="sm" data-testid="webhook-row" data-enabled={hook.enabled}>
	<div class="head">
		<div class="what">
			<Text weight="bold">{hook.name}</Text>
			<Text size="sm" tone="muted">
				{#if presetName}
					{m.webhooks_row_preset({ name: presetName })}
				{:else}
					<Text as="span" size="sm" tone="faint">{m.webhooks_preset_deleted()}</Text>
				{/if}
				<Text as="span" size="xs" tone="faint">· {m.webhooks_row_rate({ count: hook.rate_per_minute })}</Text>
			</Text>
		</div>
		<div class="actions">
			<Switch
				size="sm"
				bind:checked={() => hook.enabled, (v: boolean) => ontoggle(v)}
				disabled={busy}
				label={m.webhooks_enabled_aria({ name: hook.name })}
			/>
			<Button size="sm" disabled={busy} onclick={onrotate}>{m.webhooks_rotate()}</Button>
			<IconButton
				icon="edit"
				box="sm"
				variant="ghost"
				label={m.webhooks_edit_aria({ name: hook.name })}
				onclick={onedit}
			/>
			<IconButton
				icon="trash"
				box="sm"
				variant="ghost"
				hoverDanger
				disabled={busy}
				label={m.webhooks_delete_aria({ name: hook.name })}
				onclick={ondelete}
			/>
		</div>
	</div>
	<div class="url">
		<Text size="xs" variant="code" data-testid="webhook-url">{url}</Text>
		<CopyButton text={url} label={m.webhooks_url_copy()} box="sm" variant="ghost" />
	</div>
	<Disclosure bind:open={logOpen} size="compact">
		{#snippet header()}
			<Text size="xs" tone="faint" weight="bold">{m.webhooks_deliveries_title()}</Text>
		{/snippet}
		{#if logOpen}
			<WebhookDeliveries webhookId={hook.id} />
		{/if}
	</Disclosure>
</Card>

<style>
	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}
	.what {
		display: flex;
		flex-direction: column;
		flex: 1 1 14rem;
		min-width: 0;
	}
	.actions {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-left: auto;
	}
	.url {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
		min-width: 0;
		margin: var(--sp-2) 0;
		overflow-wrap: anywhere;
	}
</style>
