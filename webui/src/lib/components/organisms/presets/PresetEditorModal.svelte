<script lang="ts">
	import type { LaunchPreset } from '@bindings/LaunchPreset';
	import { Button, Callout, Field, Input, Modal, Textarea } from '@dorsk/tsumikit';
	import { errMessage } from '#lib/api.ts';
	import { useMachineResources } from '#lib/queries/index.ts';
	import { usePresetActions } from '#lib/queries/presets.ts';
	import { m } from '#lib/paraglide/messages.js';
	import {
		browserTimezone,
		emptyForm,
		formFromPreset,
		formProblems,
		hasProblems,
		isDispatcherTarget,
		requestFromForm,
		type PresetForm
	} from '#lib/schedules/presetForm.ts';
	import PresetEnvFields from './PresetEnvFields.svelte';
	import PresetTargetFields from './PresetTargetFields.svelte';
	import ScheduleFields from './ScheduleFields.svelte';

	let {
		preset = null,
		schedule = false,
		external = false,
		title,
		onclose,
		onsaved
	}: {
		preset?: LaunchPreset | null;
		schedule?: boolean;
		external?: boolean;
		title: string;
		onclose: () => void;
		onsaved?: (saved: LaunchPreset) => void;
	} = $props();

	const uid = $props.id();
	const actions = usePresetActions();
	const machinesQ = useMachineResources(() => true);
	const machines = $derived(machinesQ.data ?? []);
	const dispatcher = $derived(isDispatcherTarget(preset));

	// svelte-ignore state_referenced_locally
	let form = $state<PresetForm>(
		preset ? formFromPreset(preset, browserTimezone()) : emptyForm({ timezone: browserTimezone() })
	);
	let tried = $state(false);
	let busy = $state(false);
	let failure = $state('');

	const problems = $derived(formProblems(form, { schedule, dispatcher, external }));

	async function save() {
		tried = true;
		if (hasProblems(problems) || busy) return;
		busy = true;
		failure = '';
		try {
			const saved = await actions.save(preset?.id ?? null, requestFromForm(form, { schedule, base: preset, external }));
			onsaved?.(saved);
			onclose();
		} catch (e) {
			failure = errMessage(e);
		} finally {
			busy = false;
		}
	}
</script>

<Modal open {title} {onclose} {busy} size="lg">
	{#snippet body()}
		<div class="editor" data-testid="preset-editor">
			<Field
				label={m.schedules_name_label()}
				for="{uid}-name"
				error={tried && problems.name ? (schedule ? m.schedules_error_name() : m.presets_error_name()) : undefined}
			>
				<Input id="{uid}-name" bind:value={form.name} placeholder={schedule ? m.schedules_name_placeholder() : undefined} />
			</Field>
			<Field
				label={m.spawn_prompt_label()}
				for="{uid}-prompt"
				error={tried && problems.prompt ? m.schedules_error_prompt() : undefined}
			>
				<Textarea id="{uid}-prompt" rows={5} block bind:value={form.prompt} />
			</Field>

			{#if dispatcher}
				<Callout tone="info" icon="info">
					{m.schedules_dispatcher_target({ profile: preset?.profile ?? '' })}
				</Callout>
			{:else}
				<PresetTargetFields bind:form {machines} {problems} showErrors={tried} />
			{/if}

			<PresetEnvFields bind:form {problems} />

			{#if schedule}
				<ScheduleFields bind:form {problems} showErrors={tried} />
			{/if}

			{#if failure}
				<Callout tone="danger" icon="alert" role="alert">{failure}</Callout>
			{/if}
		</div>
	{/snippet}
	{#snippet footer()}
		<Button variant="ghost" disabled={busy} onclick={onclose}>{m.common_cancel()}</Button>
		<Button variant="primary" disabled={busy} onclick={save}>{m.common_save()}</Button>
	{/snippet}
</Modal>

<style>
	.editor {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
</style>
