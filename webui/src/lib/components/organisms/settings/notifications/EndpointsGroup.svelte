<script lang="ts">
	import { Button, Cluster, EmptyState, Input } from '@dorsk/tsumikit';
	import type { CreateNotificationEndpoint } from '@bindings/CreateNotificationEndpoint';
	import type { NotificationEndpoint } from '@bindings/NotificationEndpoint';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import PushSubscriptionToggle from '#lib/components/molecules/PushSubscriptionToggle.svelte';
	import { errMessage } from '#lib/api.ts';
	import { useNotificationEndpointActions, useNotificationEndpoints } from '#lib/queries/index.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import EndpointRow from './EndpointRow.svelte';
	import { emailBody, listedEndpoints, ntfyBody } from './endpoints.logic';

	const query = useNotificationEndpoints();
	const actions = useNotificationEndpointActions();
	const list = $derived(listedEndpoints(query.data));

	let busy = $state<string | null>(null);
	let ntfy = $state({ url: '', topic: '', token: '' });
	let email = $state('');
	const ntfyDraft = $derived(ntfyBody(ntfy));
	const emailDraft = $derived(emailBody(email));

	async function run(key: string, work: () => Promise<unknown>, ok: string): Promise<boolean> {
		busy = key;
		try {
			await work();
			toasts.ok(ok);
			return true;
		} catch (e) {
			toasts.error(errMessage(e));
			return false;
		} finally {
			busy = null;
		}
	}

	async function add(key: string, body: CreateNotificationEndpoint | null, reset: () => void) {
		if (body && (await run(key, () => actions.create(body), m.notif_endpoint_added()))) reset();
	}

	const test = (e: NotificationEndpoint) =>
		run(e.id, () => actions.test(e.id), m.notif_endpoint_test_sent());
	const remove = (e: NotificationEndpoint) =>
		run(e.id, () => actions.remove(e.id), m.notif_endpoint_removed());
</script>

<SettingGroup title={m.notif_set_endpoints_title()}>
	<SettingRow label={m.notif_endpoint_browser()} help={m.notif_endpoint_browser_help()} selfLabelled>
		<PushSubscriptionToggle />
	</SettingRow>
	{#if query.isPending}
		<EmptyState size="compact" loading title={m.common_loading()} />
	{:else if list.length === 0}
		<EmptyState size="compact" icon="bell" title={m.notif_set_endpoints_empty()} />
	{:else}
		{#each list as endpoint (endpoint.id)}
			<EndpointRow
				{endpoint}
				busy={busy === endpoint.id}
				ontest={() => void test(endpoint)}
				onremove={() => void remove(endpoint)}
			/>
		{/each}
	{/if}
	<SettingRow label={m.notif_endpoint_add_ntfy()} wide selfLabelled journey="notif-add-ntfy">
		<Cluster gap="var(--sp-2)" align="center">
			<Input
				id="notif-ntfy-url"
				size="sm"
				type="url"
				grow
				placeholder="https://ntfy.sh"
				aria-label={m.notif_endpoint_ntfy_url()}
				bind:value={ntfy.url}
			/>
			<Input
				id="notif-ntfy-topic"
				size="sm"
				grow
				aria-label={m.notif_endpoint_ntfy_topic()}
				bind:value={ntfy.topic}
			/>
			<Input
				id="notif-ntfy-token"
				size="sm"
				type="password"
				autocomplete="off"
				grow
				aria-label={m.notif_endpoint_ntfy_token()}
				bind:value={ntfy.token}
			/>
			<Button
				size="sm"
				variant="primary"
				disabled={!ntfyDraft || busy !== null}
				onclick={() => void add('add-ntfy', ntfyDraft, () => (ntfy = { url: '', topic: '', token: '' }))}
				data-endpoint-add="ntfy"
			>
				{m.notif_endpoint_add_ntfy()}
			</Button>
		</Cluster>
	</SettingRow>
	<SettingRow label={m.notif_endpoint_add_email()} wide selfLabelled journey="notif-add-email">
		<Cluster gap="var(--sp-2)" align="center">
			<Input
				id="notif-email-address"
				size="sm"
				type="email"
				grow
				aria-label={m.notif_endpoint_email_address()}
				bind:value={email}
			/>
			<Button
				size="sm"
				variant="primary"
				disabled={!emailDraft || busy !== null}
				onclick={() => void add('add-email', emailDraft, () => (email = ''))}
				data-endpoint-add="email"
			>
				{m.notif_endpoint_add_email()}
			</Button>
		</Cluster>
	</SettingRow>
</SettingGroup>
