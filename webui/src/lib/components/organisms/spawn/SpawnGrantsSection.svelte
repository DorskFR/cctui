<script lang="ts">
	import { Checkbox } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import SpawnAddons from './SpawnAddons.svelte';
	import SpawnContextRow from './SpawnContextRow.svelte';
	import type { SpawnForm } from './spawnForm.svelte';

	let { sf }: { sf: SpawnForm } = $props();
</script>

<SpawnAddons
	bind:labelIds={sf.form.labels}
	bind:envRows={sf.envRows}
	att={sf.att}
	allLabels={sf.allLabels}
	envInvalid={sf.badEnvKeys.length > 0}
	attachments={sf.target === 'machine'}
	machineId={sf.target === 'machine' ? sf.form.machine_id : undefined}
	labelActions={sf.actions}
/>
{#if sf.target === 'machine' && sf.form.machine_id}
	<SpawnContextRow
		bind:items={sf.form.context_items}
		bind:auto={sf.form.context_auto}
		bind:pins={sf.contextPins}
		machineId={sf.form.machine_id}
		workingDir={sf.form.working_dir}
		labels={sf.form.labels}
		adapter={sf.effectiveForm.adapter_id}
		pinnedIds={sf.profileSpec?.context_items ?? []}
	/>
{/if}
{#if sf.followupParent}
	<Checkbox bind:checked={sf.archiveSource} label={m.followup_archive_source()} />
{/if}
