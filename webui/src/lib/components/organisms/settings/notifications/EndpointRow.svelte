<script lang="ts">
	import { Badge, Button, Cluster, Timestamp } from '@dorsk/tsumikit';
	import type { NotificationEndpoint } from '@bindings/NotificationEndpoint';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import { m } from '#lib/paraglide/messages.js';
	import { channelLabel } from './labels';
	import { endpointSummary, hasToken, isServerEndpoint } from './endpoints.logic';

	let {
		endpoint,
		busy = false,
		ontest,
		onremove
	}: {
		endpoint: NotificationEndpoint;
		busy?: boolean;
		ontest: () => void;
		onremove: () => void;
	} = $props();

	const server = $derived(isServerEndpoint(endpoint));
	const summary = $derived(server ? m.notif_endpoint_server() : endpointSummary(endpoint));
</script>

<SettingRow
	label={endpoint.label ?? channelLabel(endpoint.channel)}
	help={summary}
	selfLabelled
	journey="notif-endpoint"
	journeyKey={endpoint.id}
>
	<Cluster gap="var(--sp-2)" align="center" justify="flex-end">
		{#if hasToken(endpoint)}
			<Badge>{m.notif_endpoint_has_token()}</Badge>
		{/if}
		{#if endpoint.last_ok_at}
			<span title={m.notif_endpoint_last_ok()}>
				<Timestamp value={endpoint.last_ok_at} mode="relative" />
			</span>
		{/if}
		<Button size="sm" disabled={busy} onclick={ontest} data-endpoint-test={endpoint.id}>
			{m.notif_endpoint_test()}
		</Button>
		{#if !server}
			<Button
				size="sm"
				variant="danger"
				disabled={busy}
				onclick={onremove}
				data-endpoint-remove={endpoint.id}
			>
				{m.notif_endpoint_remove()}
			</Button>
		{/if}
	</Cluster>
</SettingRow>
