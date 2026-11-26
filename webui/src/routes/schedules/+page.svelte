<script lang="ts">
	import type { LaunchPreset } from '@bindings/LaunchPreset';
	import { Button, ConfirmModal, Spinner, Text } from '@dorsk/tsumikit';
	import PageHead from '#lib/components/molecules/PageHead.svelte';
	import PresetEditorModal from '#lib/components/organisms/presets/PresetEditorModal.svelte';
	import { errMessage } from '#lib/api.ts';
	import { usePresetActions, usePresets } from '#lib/queries/presets.ts';
	import { isSchedule, withEnabled } from '#lib/schedules/presetForm.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import ScheduleRow from './ScheduleRow.svelte';

	const presets = usePresets();
	const actions = usePresetActions();
	const schedules = $derived((presets.data ?? []).filter(isSchedule));

	let editor = $state<{ preset: LaunchPreset | null } | null>(null);
	let deleting = $state<LaunchPreset | null>(null);
	let busyId = $state<string | null>(null);

	async function act(preset: LaunchPreset, work: () => Promise<unknown>, done?: string) {
		busyId = preset.id;
		try {
			await work();
			if (done) toasts.ok(done);
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busyId = null;
		}
	}

	const toggle = (preset: LaunchPreset, enabled: boolean) =>
		act(preset, () => actions.save(preset.id, withEnabled(preset, enabled)));
	const run = (preset: LaunchPreset) =>
		act(preset, () => actions.run(preset.id), m.schedules_run_queued_toast({ name: preset.name }));
	async function remove() {
		const preset = deleting;
		if (!preset) return;
		await act(preset, () => actions.remove(preset.id));
		deleting = null;
	}
</script>

<div class="page" data-journey="schedules">
	<PageHead title={m.schedules_title()}>
		<Button variant="primary" onclick={() => (editor = { preset: null })}>{m.schedules_new()}</Button>
	</PageHead>
	<Text size="sm" tone="faint" measure>{m.schedules_subtitle()}</Text>

	{#if presets.isLoading}
		<div class="msg"><Spinner /></div>
	{:else if presets.error}
		<div class="msg"><Text tone="danger">{errMessage(presets.error)}</Text></div>
	{:else if schedules.length === 0}
		<div class="msg"><Text tone="faint">{m.schedules_empty()}</Text></div>
	{:else}
		<div class="list">
			{#each schedules as preset (preset.id)}
				<ScheduleRow
					{preset}
					busy={busyId === preset.id}
					ontoggle={(enabled) => toggle(preset, enabled)}
					onrun={() => run(preset)}
					onedit={() => (editor = { preset })}
					ondelete={() => (deleting = preset)}
				/>
			{/each}
		</div>
	{/if}
</div>

{#if editor}
	<PresetEditorModal
		preset={editor.preset}
		schedule
		title={editor.preset ? m.schedules_edit_title() : m.schedules_new_title()}
		onclose={() => (editor = null)}
	/>
{/if}

<ConfirmModal
	bind:open={() => deleting !== null, (v) => { if (!v) deleting = null; }}
	tone="danger"
	title={m.schedules_delete_title()}
	message={m.schedules_delete_message({ name: deleting?.name ?? '' })}
	confirmLabel={m.common_delete()}
	cancelLabel={m.common_cancel()}
	busy={busyId !== null && busyId === deleting?.id}
	onconfirm={remove}
/>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.msg {
		display: grid;
		place-items: center;
		padding: var(--sp-6);
	}
</style>
