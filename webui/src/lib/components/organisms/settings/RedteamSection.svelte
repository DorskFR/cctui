<script lang="ts">
	import { Callout, EmptyState } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import { ApiError, errMessage } from '#lib/api.ts';
	import { useRedteamOverview } from '#lib/queries/redteamAdmin.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { versionRuns } from './redteam.logic';
	import RedteamRunRows from './RedteamRunRows.svelte';

	const overview = useRedteamOverview();
	const data = $derived(overview.data);
	const forbidden = $derived(overview.error instanceof ApiError && overview.error.status === 403);
</script>

<SettingSection id="redteam" icon="⚔" title={m.redteam_title()} description={m.redteam_desc()} admin>
	{#if overview.isPending}
		<EmptyState size="compact" loading title={m.common_loading()} />
	{:else if forbidden}
		<Callout tone="warn" title={m.redteam_forbidden()} />
	{:else if !data}
		<Callout tone="danger" title={m.redteam_load_error({ error: errMessage(overview.error) })} />
	{:else if data.versions.length === 0}
		<EmptyState size="compact" icon="lock" title={m.redteam_empty_title()} description={m.redteam_empty_desc()} />
	{:else}
		{#each data.versions as v (v.version)}
			<SettingGroup
				title={v.version === data.current_version
					? m.redteam_version_current({ version: v.version })
					: m.redteam_version({ version: v.version })}
			>
				{#each versionRuns(v) as r (r.key)}
					<RedteamRunRows label={r.label} run={r.run} />
				{/each}
			</SettingGroup>
		{/each}
	{/if}
</SettingSection>
