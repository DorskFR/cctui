<script lang="ts">
	import { Badge, Link, Spinner, Text, Timestamp } from '@dorsk/tsumikit';
	import { errMessage } from '#lib/api.ts';
	import { useWebhookDeliveries } from '#lib/queries/webhooks.ts';
	import { sessionHref } from '#lib/sessionFailureToast.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { deliveryRows } from '#lib/webhooks/deliveries.ts';

	let { webhookId }: { webhookId: string } = $props();

	const deliveries = useWebhookDeliveries(() => webhookId);
	const rows = $derived(deliveryRows(deliveries.data ?? []));
</script>

{#if deliveries.isLoading}
	<Spinner />
{:else if deliveries.error}
	<Text size="sm" tone="danger">{errMessage(deliveries.error)}</Text>
{:else if rows.length === 0}
	<Text size="sm" tone="faint">{m.webhooks_deliveries_empty()}</Text>
{:else}
	<ul class="rows" data-testid="webhook-deliveries">
		{#each rows as row (row.id)}
			<li class="row" data-outcome={row.outcome}>
				<Badge size="xs" tone={row.tone} border>{m[row.labelKey]()}</Badge>
				<Timestamp value={row.at} mode="relative" tone="faint" size="xs" />
				{#if row.deliveryId}
					<Text size="xs" tone="faint" mono>{row.deliveryId}</Text>
				{/if}
				{#if row.sessionId}
					<Link href={sessionHref(row.sessionId)}>
						{m.webhooks_delivery_session({ id: row.sessionId.slice(0, 8) })}
					</Link>
				{/if}
				{#if row.detail}
					<Text size="xs" tone={row.tone === 'danger' ? 'danger' : 'muted'}>{row.detail}</Text>
				{/if}
			</li>
		{/each}
	</ul>
{/if}

<style>
	.rows {
		display: flex;
		flex-direction: column;
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-1) 0;
		border-top: 1px solid var(--border);
	}
</style>
