<script lang="ts">
	import { Callout, EmptyState, Text } from '@dorsk/tsumikit';
	import type { StorageBucket } from '@bindings/StorageBucket';
	import type { StorageSessionSize } from '@bindings/StorageSessionSize';
	import type { StorageTableSize } from '@bindings/StorageTableSize';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import { ApiError, errMessage } from '#lib/api.ts';
	import { useStorageOverview } from '#lib/queries/storageAdmin.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { bandLabel, fmtBytes, isExact, sessionLabel } from './serverStorage.logic';
	import ServerStorageLifecycleGroup from './ServerStorageLifecycleGroup.svelte';

	const TABLES_SHOWN = 10;

	const overview = useStorageOverview();
	const data = $derived(overview.data);
	const forbidden = $derived(overview.error instanceof ApiError && overview.error.status === 403);
	const n = (v: number) => v.toLocaleString();

	function tableHelp(t: StorageTableSize): string {
		const sizes = {
			table: fmtBytes(t.table_bytes),
			indexes: fmtBytes(t.index_bytes),
			toast: fmtBytes(t.toast_bytes)
		};
		return t.row_estimate === null
			? m.settings_server_storage_table_help(sizes)
			: m.settings_server_storage_table_rows_help({ ...sizes, rows: n(t.row_estimate) });
	}

	function sessionHelp(s: StorageSessionSize): string {
		const unknown = m.settings_server_storage_unknown();
		const machine = s.machine ? (s.machine_kind ? `${s.machine} (${s.machine_kind})` : s.machine) : unknown;
		return m.settings_server_storage_session_help({
			harness: s.harness ?? unknown,
			machine,
			tool: fmtBytes(s.tool_output_bytes),
			events: n(s.events)
		});
	}
</script>

{#snippet bands(title: string, list: StorageBucket[])}
	<SettingGroup {title}>
		{#each list as b (b.key)}
			<SettingRow
				label={bandLabel(b.key)}
				help={m.settings_server_storage_tool_help({ results: n(b.results) })}
				selfLabelled
			>
				<Text size="sm" variant="code">{fmtBytes(b.bytes)}</Text>
			</SettingRow>
		{/each}
	</SettingGroup>
{/snippet}

<SettingSection
	id="server-storage"
	icon="◫"
	title={m.settings_server_storage_title()}
	description={m.settings_server_storage_desc()}
	admin
>
	{#if overview.isPending}
		<EmptyState size="compact" loading title={m.common_loading()} />
	{:else if forbidden}
		<Callout tone="warn" title={m.settings_server_storage_forbidden()} />
	{:else if !data}
		<Callout tone="danger" title={m.settings_server_storage_load_error({ error: errMessage(overview.error) })} />
	{:else}
		<SettingGroup title={m.settings_server_storage_group_database()}>
			<SettingRow
				label={m.settings_server_storage_database_label()}
				help={m.settings_server_storage_database_help()}
				selfLabelled
			>
				<Text size="sm" variant="code">{fmtBytes(data.database_bytes)}</Text>
			</SettingRow>
			<SettingRow
				label={m.settings_server_storage_images_label()}
				help={m.settings_server_storage_images_help({
					count: n(data.images.blobs),
					table: fmtBytes(data.images.table_bytes)
				})}
				selfLabelled
			>
				<Text size="sm" variant="code">{fmtBytes(data.images.image_bytes)}</Text>
			</SettingRow>
			{#each data.tables.slice(0, TABLES_SHOWN) as t (t.name)}
				<SettingRow label={t.name} help={tableHelp(t)} selfLabelled>
					<Text size="sm" variant="code">{fmtBytes(t.total_bytes)}</Text>
				</SettingRow>
			{/each}
		</SettingGroup>

		{#if data.sample}
			{@const sample = data.sample}
			<SettingGroup title={m.settings_server_storage_group_sessions()}>
				<SettingRow
					label={m.settings_server_storage_sample_note_label()}
					help={isExact(sample.percent)
						? m.settings_server_storage_sample_exact({ rows: n(sample.rows_sampled) })
						: m.settings_server_storage_sample_note({
								percent: sample.percent.toPrecision(2),
								rows: n(sample.rows_sampled)
							})}
					selfLabelled
				/>
				{#each sample.sessions as s (s.session_id)}
					<SettingRow label={sessionLabel(s)} help={sessionHelp(s)} selfLabelled>
						<Text size="sm" variant="code">{fmtBytes(s.bytes)}</Text>
					</SettingRow>
				{/each}
			</SettingGroup>
			<SettingGroup title={m.settings_server_storage_group_tools()}>
				{#each sample.tools as t (t.tool)}
					<SettingRow
						label={t.tool || m.settings_server_storage_tool_unknown()}
						help={m.settings_server_storage_tool_help({ results: n(t.results) })}
						selfLabelled
					>
						<Text size="sm" variant="code">{fmtBytes(t.bytes)}</Text>
					</SettingRow>
				{/each}
			</SettingGroup>
			{@render bands(m.settings_server_storage_group_by_size(), sample.by_size)}
			{@render bands(m.settings_server_storage_group_by_age(), sample.by_age)}
		{:else}
			<Callout
				tone="warn"
				title={m.settings_server_storage_sample_error({ error: data.sample_error ?? '' })}
			/>
		{/if}

		<ServerStorageLifecycleGroup info={data.lifecycle} />
	{/if}
</SettingSection>
