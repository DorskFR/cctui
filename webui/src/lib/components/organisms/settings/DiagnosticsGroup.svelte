<script lang="ts">
	import { Button, Text } from '@dorsk/tsumikit';
	import { useQueryClient } from '@tanstack/svelte-query';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import NetStatsChip from '#lib/components/molecules/NetStatsChip.svelte';
	import UpdateModal from '#lib/components/organisms/UpdateModal.svelte';
	import { endpoints, qk, useVersion } from '#lib/queries/index.ts';
	import { releaseChannel } from '#lib/releaseChannel.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { EVENTS_PAGE } from './settings.logic';
	import { m } from '#lib/paraglide/messages.js';

	const selfUpdate = $derived(hasFeature('self_update'));
	const version = useVersion();
	const qc = useQueryClient();

	// The server probes GitHub every 6h; this asks it to go now and swaps the
	// cached `/version` payload, so the header's update arrow follows at once.
	let updateOpen = $state(false);
	let updateChecking = $state(false);
	async function checkForUpdate() {
		updateChecking = true;
		try {
			const info = await endpoints.refreshVersion();
			qc.setQueryData(qk.version, info);
			toasts.ok(
				info.latest_version
					? m.settings_version_available({ version: info.latest_version })
					: m.settings_version_up_to_date()
			);
		} catch (e) {
			toasts.error(m.settings_version_check_failed({ error: e instanceof Error ? e.message : String(e) }));
		} finally {
			updateChecking = false;
		}
	}
</script>

<SettingGroup title={m.settings_group_diagnostics()}>
	<SettingRow
		label={m.settings_version_title()}
		help={selfUpdate ? m.settings_version_check_help() : undefined}
		selfLabelled
	>
		<div class="ver">
			{#if version.data}
				<Text size="sm" variant="code">srv {version.data.version}</Text>
				{#if releaseChannel(version.data.version) === 'beta'}
					<span title={m.release_channel_beta_title()}>
						<Text size="sm" tone="accent" variant="code" weight="bold">{m.release_channel_beta()}</Text>
					</span>
				{/if}
				<Text size="sm" variant="code">ui {__CLIENT_VERSION__}</Text>
				{#if selfUpdate && version.data.latest_version}
					<Button size="sm" variant="ghost" chip onclick={() => (updateOpen = true)}>
						<Text size="sm" variant="code" tone="danger">↑ v{version.data.latest_version}</Text>
					</Button>
				{/if}
			{/if}
			{#if selfUpdate}
				<Button size="sm" disabled={updateChecking} onclick={checkForUpdate}>
					{updateChecking ? m.settings_version_checking() : m.settings_version_check()}
				</Button>
			{/if}
		</div>
	</SettingRow>
	<SettingRow label={m.net_stats_title()} help={m.settings_net_stats_help()} selfLabelled>
		<NetStatsChip />
	</SettingRow>
	<SettingRow label={m.settings_nav_events()} help={m.settings_events_link_help()} selfLabelled>
		<Button size="sm" variant="ghost" as="a" href="/settings/{EVENTS_PAGE}">{m.settings_events_open()}</Button>
	</SettingRow>
</SettingGroup>

{#if updateOpen && version.data?.latest_version}
	<UpdateModal
		latestVersion={version.data.latest_version}
		latestUrl={version.data.latest_url ?? version.data.repo_url}
		selfUpdateReady={version.data.self_update_ready}
		selfUpdateHook={version.data.self_update_hook}
		onclose={() => (updateOpen = false)}
	/>
{/if}

<style>
	.ver {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
