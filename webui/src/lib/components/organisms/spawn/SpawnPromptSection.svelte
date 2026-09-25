<script lang="ts">
	import MachineFields from './MachineFields.svelte';
	import DispatchFields from './DispatchFields.svelte';
	import ProfileList from './ProfileList.svelte';
	import type { SpawnForm } from './spawnForm.svelte';

	let { sf }: { sf: SpawnForm } = $props();
</script>

{#if sf.target === 'dispatch'}
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
		onfiles={sf.addFiles}
	/>
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
		oncreate={() => sf.createProfile()}
		onsave={(id, name, spec) => sf.saveProfile(id, name, spec)}
		ondelete={(id) => sf.deleteProfile(id)}
	/>
{/if}
