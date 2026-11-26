<script lang="ts">
	import { tick } from 'svelte';
	import { Checkbox } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';
	import SpawnAddons from './SpawnAddons.svelte';
	import SpawnContextRow from './SpawnContextRow.svelte';
	import type { SpawnForm } from './spawnForm.svelte';

	let { sf }: { sf: SpawnForm } = $props();

	let contextOpen = $state(false);
	const contextAvailable = $derived(sf.target === 'machine' && !!sf.form.machine_id && sf.canUseContext);
	const contextShown = $derived(
		contextOpen ||
			sf.form.context_items.length > 0 ||
			sf.contextPins.length > 0 ||
			(sf.profileSpec?.context_items?.length ?? 0) > 0
	);

	async function openContext() {
		contextOpen = true;
		await tick();
		document.getElementById('sp-context')?.focus();
	}
</script>

<SpawnAddons
	bind:labelIds={sf.form.labels}
	bind:envRows={sf.envRows}
	att={sf.att}
	allLabels={sf.allLabels}
	labelable={sf.canLabel}
	envInvalid={sf.badEnvKeys.length > 0}
	attachments={sf.target === 'machine' && sf.canAttach}
	machineId={sf.target === 'machine' ? sf.form.machine_id : undefined}
	labelActions={sf.actions}
	oncontext={contextAvailable && !contextShown ? openContext : undefined}
/>
{#if contextAvailable && contextShown}
	<SpawnContextRow
		bind:items={sf.form.context_items}
		bind:pins={sf.contextPins}
		machineId={sf.form.machine_id}
		workingDir={sf.form.working_dir}
		labels={sf.labelIds}
		adapter={sf.effectiveForm.adapter_id}
		pinnedIds={sf.profileSpec?.context_items ?? []}
	/>
{/if}
{#if sf.followupParent}
	<Checkbox bind:checked={sf.archiveSource} label={m.followup_archive_source()} />
{/if}
