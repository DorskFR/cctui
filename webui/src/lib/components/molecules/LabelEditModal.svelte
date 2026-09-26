<script lang="ts">
	import { untrack } from 'svelte';
	import type { Label } from '@bindings/Label';
	import { Badge, Button, Field, Input, Modal } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import Swatch from '$lib/components/atoms/Swatch.svelte';
	import { LABEL_HUES, labelTint, storedHue, hueToColor } from '$lib/labels';

	// Rename + recolor (+ delete) a single label. The propagation guard keeps a
	// backdrop/close click from bubbling to a clickable ancestor (e.g. the session
	// Card the picker lives on).
	let {
		label,
		onUpdate,
		onDelete,
		onclose
	}: {
		label: Label;
		onUpdate?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDelete?: (labelId: string) => void | Promise<void>;
		onclose: () => void;
	} = $props();

	let editName = $state(untrack(() => label.name));
	let editHue = $state<number | null>(untrack(() => storedHue(label.color)));
	let editBusy = $state(false);
	let editError = $state('');

	async function saveEdit() {
		if (!onUpdate || editBusy) return;
		const name = editName.trim();
		if (!name) {
			editError = m.sessions_name_required();
			return;
		}
		editBusy = true;
		editError = '';
		try {
			const patch: { name?: string; color?: string } = { color: hueToColor(editHue) };
			if (name !== label.name) patch.name = name;
			await onUpdate(label.id, patch);
			onclose();
		} catch (e) {
			editError = e instanceof Error ? e.message : m.sessions_label_save_failed();
		} finally {
			editBusy = false;
		}
	}

	async function deleteEditing() {
		if (!onDelete || editBusy) return;
		editBusy = true;
		editError = '';
		try {
			await onDelete(label.id);
			onclose();
		} catch (e) {
			editError = e instanceof Error ? e.message : m.sessions_label_delete_failed();
		} finally {
			editBusy = false;
		}
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<span
	class="modal-host"
	onpointerdown={(e) => e.stopPropagation()}
	onclick={(e) => e.stopPropagation()}
>
	<Modal title={m.sessions_edit_label()} size="sm" {onclose}>
		{#snippet body()}
			<div class="edit-form">
				<Field label={m.sessions_field_name()}>
					<Input
						bind:value={editName}
						maxlength={40}
						placeholder={m.sessions_label_name_placeholder()}
						onkeydown={(e: KeyboardEvent) => {
							if (e.key === 'Enter') {
								e.preventDefault();
								saveEdit();
							}
						}}
					/>
				</Field>
				<Field label={m.sessions_field_color()}>
					<div class="hues" role="radiogroup" aria-label={m.sessions_label_color_aria()}>
						<Swatch
							hue={null}
							active={editHue == null}
							title={m.sessions_color_auto_title()}
							aria-label={m.sessions_color_auto_aria()}
							onclick={() => (editHue = null)}>A</Swatch
						>
						{#each LABEL_HUES as h (h)}
							<Swatch
								hue={h}
								active={editHue === h}
								title={m.sessions_hue({ hue: h })}
								aria-label={m.sessions_hue({ hue: h })}
								onclick={() => (editHue = h)}
							/>
						{/each}
					</div>
				</Field>
				<div class="preview-row">
					<span class="preview-label">{m.sessions_preview()}</span>
					<Badge
						class="label"
						style="{labelTint({ name: editName || m.sessions_label_placeholder_word(), color: hueToColor(editHue) })};border-radius:var(--r-sm)"
					>
						<span class="opt-name">{editName || m.sessions_label_placeholder_word()}</span>
					</Badge>
				</div>
				{#if editError}<p class="edit-error">{editError}</p>{/if}
			</div>
		{/snippet}
		{#snippet footer()}
			{#if onDelete}
				<Button variant="danger" disabled={editBusy} onclick={deleteEditing}>{m.common_delete()}</Button>
			{/if}
			<Button
				variant="primary"
				block
				loading={editBusy}
				disabled={editBusy || !editName.trim()}
				onclick={saveEdit}
			>
				{m.common_save()}
			</Button>
		{/snippet}
	</Modal>
</span>

<style>
	/* No box of its own — purely an event boundary so the dialog's clicks don't
	   bubble to a clickable ancestor. Events still follow the DOM tree under
	   display:contents. */
	.modal-host {
		display: contents;
	}
	.edit-form {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 16rem;
	}
	.hues {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
	.preview-row {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.preview-label {
		color: var(--text-muted);
		font-size: var(--fs-sm);
	}
	.opt-name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.edit-error {
		margin: 0;
		color: var(--danger, var(--text));
		font-size: var(--fs-sm);
	}
</style>
