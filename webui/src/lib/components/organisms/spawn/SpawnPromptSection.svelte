<script lang="ts">
	import MachineFields from './MachineFields.svelte';
	import DispatchFields from './DispatchFields.svelte';
	import SpawnDictation from './SpawnDictation.svelte';
	import ProfileList from './ProfileList.svelte';
	import type { SpawnForm } from './spawnForm.svelte';
	import { createProfile, deleteProfile, saveProfile } from './profileCrud';
	import { Callout } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';
	import CctuiverseSpawnNotice from '#lib/components/molecules/CctuiverseSpawnNotice.svelte';
	import { findInvite } from '#lib/cctuiverse.ts';
	import { cctuiverseConfig, loadCctuiverseConfig } from '#lib/cctuiverseConfig.svelte.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { settingsHref } from '../settings/settings.logic';

	let { sf }: { sf: SpawnForm } = $props();

	$effect(() => {
		void loadCctuiverseConfig();
	});
	const promptTools = $derived(hasFeature('voice') ? dictation : undefined);
	const hasInvite = $derived(hasFeature('federation') && findInvite(sf.form.prompt) !== null);
</script>

{#snippet dictation()}
	<SpawnDictation {sf} />
{/snippet}

{#if sf.target === 'dispatch'}
	{#if hasInvite}
		<Callout tone="warn" style="flex-basis:100%">{m.cctuiverse_dispatch_unsupported()}</Callout>
	{/if}
	<DispatchFields
		bind:form={sf.form}
		dispatcherIds={sf.dispatcherIds}
		accounts={sf.allAccounts}
		onsubmit={sf.submit}
		{promptTools}
	/>
{:else}
	<MachineFields
		bind:form={sf.form}
		machines={sf.machineList}
		recentDirs={sf.recentDirs}
		onsubmit={sf.submit}
		att={sf.att}
		bind:promptEl={sf.promptEl}
		cwdCheck={sf.cwdCheck}
		machineName={sf.machineName}
		{promptTools}
	/>
	{#if hasInvite && !cctuiverseConfig.enabled}
		<Callout tone="warn" style="flex-basis:100%">{m.cctuiverse_disabled_invite()}</Callout>
	{:else if hasInvite}
		<CctuiverseSpawnNotice bind:label={sf.joinLabel} placeholder={sf.form.name.trim()} />
	{/if}
	{#if sf.canUseProfiles}
		<ProfileList
			profiles={sf.profiles}
			bind:selectedId={sf.selectedProfileId}
			bind:oneOff={sf.oneOff}
			accounts={sf.allAccounts}
			pools={sf.allPools}
			usage={sf.allUsage}
			machineId={sf.form.machine_id}
			busy={sf.busy}
			oncreate={(name, spec) => createProfile(sf, name, spec)}
			onsave={(id, name, spec) => saveProfile(sf, id, name, spec)}
			ondelete={(id) => deleteProfile(sf, id)}
			manageHref={settingsHref('sessions', 'launch')}
			onmanage={sf.leave}
		/>
	{/if}
{/if}
