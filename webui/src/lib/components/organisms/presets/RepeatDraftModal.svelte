<script lang="ts">
	// The spawn form's "Repeat…" schedule action: the draft becomes a schedule
	// with its whole payload; only the name and the repeat are asked here.
	import { Button, Field, Input, Modal } from '@dorsk/tsumikit';
	import type { SpawnForm } from '../spawn/spawnForm.svelte';
	import { m } from '#lib/paraglide/messages.js';
	import { cronFromRepeat } from '#lib/schedules/cron.ts';
	import { browserTimezone, emptyForm, formProblems, hasProblems, type PresetForm } from '#lib/schedules/presetForm.ts';
	import ScheduleFields from './ScheduleFields.svelte';

	let { sf, onclose }: { sf: SpawnForm; onclose: () => void } = $props();

	const uid = $props.id();
	let form = $state<PresetForm>(emptyForm({ timezone: browserTimezone() }));
	// svelte-ignore state_referenced_locally
	let name = $state(sf.form.name.trim());
	let tried = $state(false);

	const problems = $derived.by(() => {
		const { name: _name, prompt: _prompt, ...rest } = formProblems(form, { schedule: true, dispatcher: true });
		return rest;
	});

	async function save() {
		tried = true;
		if (hasProblems(problems) || sf.busy) return;
		const done = await sf.scheduleRepeat({
			name: name.trim() || null,
			cron: cronFromRepeat(form.repeat),
			timezone: form.timezone || null,
			overlap: form.overlap,
			enabled: form.enabled
		});
		if (done) onclose();
	}
</script>

<Modal open title={m.schedules_repeat_draft_title()} {onclose} busy={sf.busy}>
	{#snippet body()}
		<div class="repeat" data-testid="repeat-draft">
			<Field label={m.schedules_name_label()} for="{uid}-name">
				<Input id="{uid}-name" bind:value={name} placeholder={m.schedules_name_placeholder()} />
			</Field>
			<ScheduleFields bind:form {problems} showErrors={tried} />
		</div>
	{/snippet}
	{#snippet footer()}
		<Button variant="ghost" disabled={sf.busy} onclick={onclose}>{m.common_cancel()}</Button>
		<Button variant="primary" loading={sf.busy} disabled={sf.busy} onclick={save}>
			{m.schedules_repeat_draft_save()}
		</Button>
	{/snippet}
</Modal>

<style>
	.repeat {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
</style>
