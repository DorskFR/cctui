<script lang="ts">
	import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
	import { AutoGrid, Callout, Field, FilterInput, Link, OptionButton, Select, Text, WorkingDir } from '@dorsk/tsumikit';
	import type { Query, SelectOption } from '@dorsk/tsumikit';
	import BrandLogo from '#lib/components/atoms/BrandLogo.svelte';
	import MachinePicker from '#lib/components/molecules/MachinePicker.svelte';
	import PermissionModes from '#lib/components/organisms/spawn/PermissionModes.svelte';
	import { dirFromQuery, makeCwdSchema } from '#lib/components/organisms/spawn/cwdSchema.ts';
	import { fsLiveness } from '#lib/components/organisms/spawn/cwdGitInfo.ts';
	import { harnessModelsFallback } from '#lib/domainTables.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { harnessTable } from '#lib/harnesses.svelte.ts';
	import { familyAccent, pickableHarnesses } from '#lib/harnesses.ts';
	import { modelHintText, withCurrentModel } from '#lib/harnessModels.ts';
	import { useHarnessModels, useHarnesses, useMachineAdapters } from '#lib/queries/index.ts';
	import { m } from '#lib/paraglide/messages.js';
	import type { PresetForm, PresetFormProblems } from '#lib/schedules/presetForm.ts';

	let {
		form = $bindable(),
		machines,
		problems,
		showErrors = false
	}: {
		form: PresetForm;
		machines: MachineResourcesRow[];
		problems: PresetFormProblems;
		showErrors?: boolean;
	} = $props();

	const uid = $props.id();

	useHarnesses();
	const machineAdapters = useMachineAdapters(() => form.machine_id);
	const enabledIds = $derived(
		machineAdapters.data?.filter((a) => a.enabled).map((a) => a.adapter_id) ?? null
	);
	const harnesses = $derived(pickableHarnesses(harnessTable(), enabledIds));

	$effect(() => {
		if (!form.machine_id && machines.length) form.machine_id = machines[0].machine_id;
	});
	$effect(() => {
		if (!form.adapter_id && harnesses.length) form.adapter_id = harnesses[0].id;
	});

	const liveness = $derived(fsLiveness(machines.find((mc) => mc.machine_id === form.machine_id)));
	const cwdSchema = makeCwdSchema(
		() => (liveness === 'offline' || !hasFeature('files') ? '' : form.machine_id),
		() => [],
		m.spawn_cwd_label()
	);
	// svelte-ignore state_referenced_locally
	let cwdRaw = $state(form.working_dir);
	// svelte-ignore state_referenced_locally
	let lastDir = form.working_dir;
	function onCwdChange(q: Query) {
		const dir = dirFromQuery(q);
		if (dir === lastDir) return;
		lastDir = dir;
		form.working_dir = dir;
	}

	const harnessModels = useHarnessModels(
		() => form.adapter_id,
		() => form.machine_id,
		() => form.model
	);
	const modelOptions = $derived.by<SelectOption[]>(() => {
		const list = harnessModels.data?.models ?? harnessModelsFallback(form.adapter_id).models;
		const out: SelectOption[] = withCurrentModel(list, form.model).map((o) => ({
			value: o.v,
			label: o.v ? o.label : m.spawn_model_default(),
			hint: modelHintText(o.hint),
			disabled: o.disabled
		}));
		if (!out.some((o) => o.value === '')) out.unshift({ value: '', label: m.spawn_model_default() });
		return out;
	});

	function pickHarness(id: string) {
		if (id === form.adapter_id) return;
		form.adapter_id = id;
		form.model = '';
	}

	const cwdError = $derived(
		!showErrors
			? undefined
			: problems.machine
				? m.schedules_error_machine()
				: problems.working_dir === 'required'
					? m.schedules_error_working_dir()
					: problems.working_dir === 'absolute'
						? m.schedules_error_working_dir_absolute()
						: undefined
	);
</script>

<div class="target" data-testid="preset-target">
	{#if machines.length === 0}
		<Callout tone="warn" icon="info">
			{m.spawn_no_machines_hint()}
			<Link href="/">{m.nav_overview()}</Link>
		</Callout>
	{/if}

	<Field label={m.spawn_cwd_label()} for="{uid}-cwd" error={cwdError}>
		<FilterInput
			id="{uid}-cwd"
			key="cwd"
			schema={cwdSchema}
			bind:value={cwdRaw}
			icon={null}
			showClear={false}
			placeholder="/home/user/project"
			title={form.working_dir || undefined}
			onchange={onCwdChange}
		>
			{#snippet inline()}
				<MachinePicker bind:value={form.machine_id} {machines} label={m.spawn_machine_label()} />
			{/snippet}
			{#snippet display()}
				<WorkingDir path={form.working_dir} shrink minLeaf={12} />
			{/snippet}
		</FilterInput>
	</Field>

	<Field
		label={m.spawn_field_harness()}
		error={showErrors && problems.adapter ? m.schedules_error_adapter() : undefined}
	>
		<AutoGrid min="8rem" maxCols={2} gap="var(--sp-2)" role="radiogroup" aria-label={m.spawn_profile_harness_aria()}>
			{#each harnesses as h (h.id)}
				<OptionButton
					row
					selected={form.adapter_id === h.id}
					role="radio"
					aria-checked={form.adapter_id === h.id}
					style="--opt-accent: {familyAccent(h.family)}"
					onclick={() => pickHarness(h.id)}
				>
					<BrandLogo adapter={h.id} size={18} />
					<Text>{h.label}</Text>
				</OptionButton>
			{/each}
		</AutoGrid>
	</Field>

	<Field label={m.spawn_field_model()} for="{uid}-model">
		<Select id="{uid}-model" options={modelOptions} bind:value={form.model} />
	</Field>

	<PermissionModes
		harness={form.adapter_id || null}
		value={form.permission_mode}
		onpick={(v) => (form.permission_mode = v)}
	/>
	{#if showErrors && problems.permission_mode}
		<Text size="xs" tone="danger" role="alert">{m.presets_error_permission_guarded()}</Text>
	{/if}
</div>

<style>
	.target {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
</style>
