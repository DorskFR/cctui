<script lang="ts">
	import { Badge, Timestamp } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import MachineAdaptersGroup from './MachineAdaptersGroup.svelte';
	import { useAllMachines, useMachineResources, useMe } from '#lib/queries/index.ts';
	import { ownMachines } from './machineAdapters.logic';
	import { m } from '#lib/paraglide/messages.js';

	let { isAdmin = false }: { isAdmin?: boolean } = $props();

	const me = useMe();
	const all = useAllMachines(() => isAdmin);
	const own = useMachineResources(() => !isAdmin);
	const rows = $derived(ownMachines(me.data?.user_id, isAdmin, all.data, own.data));
</script>

<SettingSection id="machines" icon="▣" title={m.settings_sec_machines()} description={m.settings_machines_desc()}>
	<SettingGroup title={m.settings_machines_mine()}>
		{#each rows as r (r.id)}
			<SettingRow label={r.name} selfLabelled journey="own-machine" journeyKey={r.id}>
				<span class="state">
					<Badge size="sm" tone={r.liveness === 'online' ? 'success' : 'muted'} border>{r.liveness}</Badge>
					{#if r.lastSeen}
						<Timestamp value={r.lastSeen} mode="relative" size="xs" tone="faint" />
					{/if}
				</span>
			</SettingRow>
		{:else}
			<SettingRow label={m.settings_machines_none()} help={m.settings_machines_none_help()} selfLabelled wide />
		{/each}
	</SettingGroup>
	{#if !isAdmin}
		<MachineAdaptersGroup />
	{/if}
</SettingSection>

<style>
	.state {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
