<script lang="ts">
	// The spawn form's own fields, bound to a schedule's template: where and
	// what, the kit, labels, files, env and context. Only the repeat is not here.
	import { Button, Callout } from '@dorsk/tsumikit';
	import {
		useAccountPools,
		useAccounts,
		useAllAccountsUsage,
		useLabels,
		useMachineResources,
		useRecentDirs,
		useSessionActions
	} from '#lib/queries/index.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { normalizeDir } from '#lib/drafts.ts';
	import type { PromptAttachments } from '#lib/promptAttachments.svelte.ts';
	import KitFields from '#lib/components/organisms/spawn/KitFields.svelte';
	import MachineFields from '#lib/components/organisms/spawn/MachineFields.svelte';
	import SpawnAddons from '#lib/components/organisms/spawn/SpawnAddons.svelte';
	import SpawnAutoArchive from '#lib/components/organisms/spawn/SpawnAutoArchive.svelte';
	import SpawnContextRow from '#lib/components/organisms/spawn/SpawnContextRow.svelte';
	import { m } from '#lib/paraglide/messages.js';
	import type { PresetForm, PresetFormProblems } from '#lib/schedules/presetForm.ts';

	let {
		form = $bindable(),
		att,
		problems,
		showErrors = false,
		schedule = false,
		storedFiles = [],
		clearFiles = $bindable(false)
	}: {
		form: PresetForm;
		att: PromptAttachments;
		problems: PresetFormProblems;
		showErrors?: boolean;
		schedule?: boolean;
		/** Names of the files the preset already gives every run. */
		storedFiles?: string[];
		clearFiles?: boolean;
	} = $props();

	const machinesQ = useMachineResources(() => true);
	const accountsQ = useAccounts();
	const poolsQ = useAccountPools();
	const usageQ = useAllAccountsUsage();
	const labelsQ = useLabels();
	const labelActions = useSessionActions();
	const dirsQ = useRecentDirs(() => form.spawn.machine_id);
	const recentDirs = $derived([...new Set((dirsQ.data ?? []).map(normalizeDir))]);
	let promptEl = $state<HTMLTextAreaElement | null>(null);
	$effect(() => {
		const list = machinesQ.data ?? [];
		if (!form.spawn.machine_id && list.length) form.spawn.machine_id = list[0].machine_id;
	});

	const errors = $derived(
		showErrors
			? [
					problems.name && (schedule ? m.schedules_error_name() : m.presets_error_name()),
					problems.prompt && m.schedules_error_prompt(),
					problems.machine && m.schedules_error_machine(),
					problems.working_dir === 'required' && m.schedules_error_working_dir(),
					problems.working_dir === 'absolute' && m.schedules_error_working_dir_absolute(),
					problems.adapter && m.schedules_error_adapter(),
					problems.permission_mode && m.presets_error_permission_guarded()
				].flatMap((e) => (e ? [e] : []))
			: []
	);
</script>

<div class="fields" data-testid="preset-spawn-fields">
	<MachineFields bind:form={form.spawn} machines={machinesQ.data ?? []} {recentDirs} {att} bind:promptEl />
	<KitFields
		bind:draft={form.kit}
		accounts={accountsQ.data ?? []}
		pools={poolsQ.data ?? []}
		usage={usageQ.data ?? []}
		machineId={form.spawn.machine_id}
		idSuffix="preset"
	/>
	<SpawnAddons
		bind:labelIds={form.spawn.labels}
		bind:envRows={form.env}
		{att}
		allLabels={labelsQ.data?.labels ?? []}
		labelable={hasFeature('organising')}
		envInvalid={!!problems.env}
		attachments={hasFeature('uploads')}
		machineId={form.spawn.machine_id}
		{labelActions}
	/>
	{#if storedFiles.length && !att.files.length}
		<div class="stored" data-testid="preset-stored-files">
			<span>
				{clearFiles ? m.schedules_files_cleared() : m.schedules_files_stored({ names: storedFiles.join(', ') })}
			</span>
			<Button size="sm" variant="ghost" onclick={() => (clearFiles = !clearFiles)}>
				{clearFiles ? m.schedules_files_keep() : m.schedules_files_clear()}
			</Button>
		</div>
	{/if}
	{#if hasFeature('context') && form.spawn.machine_id}
		<SpawnContextRow
			bind:items={form.spawn.context_items}
			machineId={form.spawn.machine_id}
			workingDir={form.spawn.working_dir}
			labels={form.spawn.labels}
			adapter={form.kit.harness}
		/>
	{/if}
	<SpawnAutoArchive bind:checked={form.spawn.auto_archive} />
	{#if errors.length}
		<Callout tone="danger" icon="alert" role="alert">
			{#each errors as error (error)}<div>{error}</div>{/each}
		</Callout>
	{/if}
</div>

<style>
	.fields {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.stored {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
		font-size: var(--fs-sm);
		color: var(--text-muted);
	}
</style>
