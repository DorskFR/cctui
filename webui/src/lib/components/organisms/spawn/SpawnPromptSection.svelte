<script lang="ts">
	import MachineFields from './MachineFields.svelte';
	import DispatchFields from './DispatchFields.svelte';
	import ProfileList from './ProfileList.svelte';
	import type { SpawnForm } from './spawnForm.svelte';
	import { createProfile, deleteProfile, saveProfile } from './profileCrud';
	import { Callout } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import CctuiverseSpawnNotice from '$lib/components/molecules/CctuiverseSpawnNotice.svelte';
	import { findInvite } from '$lib/cctuiverse';
	import { cctuiverseConfig, loadCctuiverseConfig } from '$lib/cctuiverseConfig.svelte';

	let { sf }: { sf: SpawnForm } = $props();

	$effect(() => {
		void loadCctuiverseConfig();
	});
	const hasInvite = $derived(findInvite(sf.form.prompt) !== null);
</script>

{#if sf.target === 'dispatch'}
	{#if hasInvite}
		<Callout tone="warn" style="flex-basis:100%">{m.cctuiverse_dispatch_unsupported()}</Callout>
	{/if}
	<DispatchFields
		bind:form={sf.form}
		dispatcherIds={sf.dispatcherIds}
		accounts={sf.allAccounts}
		onsubmit={sf.submit}
	/>
{:else}
	<MachineFields
		bind:form={sf.form}
		machines={sf.machineList}
		recentDirs={sf.recentDirs}
		onsubmit={sf.submit}
		att={sf.att}
		bind:promptEl={sf.promptEl}
	/>
	{#if hasInvite && !cctuiverseConfig.enabled}
		<Callout tone="warn" style="flex-basis:100%">{m.cctuiverse_disabled_invite()}</Callout>
	{:else if hasInvite}
		<CctuiverseSpawnNotice bind:label={sf.joinLabel} placeholder={sf.form.name.trim()} />
	{/if}
	<ProfileList
		profiles={sf.profiles}
		bind:selectedId={sf.selectedProfileId}
		bind:oneOff={sf.oneOff}
		accounts={sf.allAccounts}
		pools={sf.allPools}
		usage={sf.allUsage}
		usageRaw={sf.usageRaw}
		machineId={sf.form.machine_id}
		busy={sf.busy}
		oncreate={() => createProfile(sf)}
		onsave={(id, name, spec) => saveProfile(sf, id, name, spec)}
		ondelete={(id) => deleteProfile(sf, id)}
	/>
{/if}
