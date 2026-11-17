<script lang="ts">
	import { DataTable, type Column } from '@dorsk/tsumikit';
	import type { NotificationSettings } from '@bindings/NotificationSettings';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import {
		NOTIFICATION_KINDS,
		effectiveChannelEnabled,
		effectiveStage
	} from '#lib/notificationPrefs.ts';
	import { settings } from '#lib/settings.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { channelLabel, kindLabel } from './labels';
	import StageCell from './StageCell.svelte';

	let { resolved }: { resolved?: NotificationSettings } = $props();

	type Row = { kind: string };
	const rows: Row[] = NOTIFICATION_KINDS.map((kind) => ({ kind }));
	const prefs = $derived(settings.state.notifications);

	const columns: Column<Row>[] = $derived([
		{ key: 'kind', label: m.notif_set_matrix_kind(), get: (r: Row) => kindLabel(r.kind) },
		{ key: 'in_app', label: channelLabel('in_app') },
		{ key: 'web_push', label: channelLabel('web_push') },
		{ key: 'ntfy', label: channelLabel('ntfy') },
		{ key: 'email', label: channelLabel('email') }
	]);

	const off = (channel: string) =>
		!prefs.enabled || !effectiveChannelEnabled(prefs, resolved, channel);
</script>

{#snippet cell(row: Row, channel: string)}
	<StageCell
		kind={row.kind}
		{channel}
		stage={effectiveStage(prefs, resolved, row.kind, channel)}
		disabled={off(channel)}
		onchange={(stage) => settings.setNotificationStage(row.kind, channel, stage)}
	/>
{/snippet}
{#snippet inApp(row: Row)}{@render cell(row, 'in_app')}{/snippet}
{#snippet webPush(row: Row)}{@render cell(row, 'web_push')}{/snippet}
{#snippet ntfy(row: Row)}{@render cell(row, 'ntfy')}{/snippet}
{#snippet email(row: Row)}{@render cell(row, 'email')}{/snippet}

<SettingGroup>
	<SettingRow label={m.notif_set_matrix_title()} help={m.notif_set_matrix_help()} wide selfLabelled>
		<DataTable
			{columns}
			{rows}
			rowKey={(r: Row) => r.kind}
			size="sm"
			cellSnippets={{ in_app: inApp, web_push: webPush, ntfy, email }}
		/>
	</SettingRow>
</SettingGroup>
