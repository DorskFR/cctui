<script lang="ts">
	import type { Label } from '@bindings/Label';
	import type { LabelOwner } from '$lib/labels';
	import { Badge, Button, FileButton, Icon, Popover } from '@dorsk/tsumikit';
	import { labelTint, hueToColor } from '$lib/labels';
	import AttachmentList from '$lib/components/molecules/AttachmentList.svelte';
	import type { PromptAttachments } from '$lib/promptAttachments.svelte';
	import LabelMenu from '$lib/components/molecules/LabelMenu.svelte';
	import EnvSecretsField from './EnvSecretsField.svelte';
	import type { EnvRow } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		labelIds = $bindable(),
		envRows = $bindable(),
		att,
		allLabels,
		envInvalid,
		attachments,
		machineId,
		labelActions
	}: {
		labelIds: string[];
		envRows: EnvRow[];
		att: PromptAttachments;
		allLabels: Label[];
		envInvalid: boolean;
		attachments: boolean;
		/** The target machine: a label created here joins its owner's labels. */
		machineId?: string;
		labelActions: {
			createLabel: (name: string, color: string, owner?: LabelOwner) => Promise<Label>;
			updateLabel: (id: string, patch: { name?: string; color?: string }) => Promise<Label>;
			deleteLabel: (id: string) => Promise<void>;
		};
	} = $props();

	const selectedLabels = $derived(allLabels.filter((l) => labelIds.includes(l.id)));
	const attachedLabelIds = $derived(new Set(labelIds));
	function toggleLabel(l: Label) {
		labelIds = labelIds.includes(l.id) ? labelIds.filter((x) => x !== l.id) : [...labelIds, l.id];
	}
	async function createAndAttach(name: string) {
		if (!name.trim()) return;
		const owner = machineId ? { machine_id: machineId } : undefined;
		const label = await labelActions.createLabel(name, hueToColor(null), owner);
		if (!labelIds.includes(label.id)) labelIds = [...labelIds, label.id];
	}

	// The kit keeps a popover's panel mounted after its first open, so LabelMenu's
	// mount-time autofocus only fires once; every reopen refocuses explicitly.
	let panel = $state<LabelMenu>();
	const addEnvRow = () => (envRows = [...envRows, { key: '', value: '' }]);
</script>

<div class="addons">
	<div class="addon-row" role="group" aria-label={m.spawn_optional_settings()}>
		<!-- The panel renders in the browser top layer, so it is neither clipped by
		     the Spawn Modal's <dialog> nor by its scrolling body. -->
		<Popover
			label={m.spawn_labels_aria()}
			placement="bottom-start"
			role="menu"
			haspopup="menu"
			variant="ghost"
			size="sm"
			panelStyle="max-height:calc(100dvh - 1rem);overflow-y:auto;overflow-x:hidden"
			onopen={() => panel?.focusSearch()}
		>
			{#snippet trigger()}
				<Icon name="plus" />{m.spawn_add_label()}
			{/snippet}
			<LabelMenu
				bind:this={panel}
				labels={allLabels}
				selectedIds={attachedLabelIds}
				cap={5}
				autofocus
				onToggle={toggleLabel}
				onCreate={createAndAttach}
				onUpdate={(labelId, patch) => labelActions.updateLabel(labelId, patch)}
				onDelete={(labelId) => labelActions.deleteLabel(labelId)}
			/>
		</Popover>
		{#if attachments}
			<FileButton
				label={m.spawn_add_files()}
				icon="plus"
				variant="ghost"
				size="sm"
				multiple
				onfiles={att.add}
			/>
		{/if}
		<Button variant="ghost" size="sm" onclick={addEnvRow}><Icon name="plus" />{m.spawn_add_env_vars()}</Button>
	</div>

	{#if selectedLabels.length}
		<div class="addon-labels">
			{#each selectedLabels as l (l.id)}
				<Badge
					style="{labelTint(l)};border-radius:var(--r-sm)"
					removable
					onremove={() => (labelIds = labelIds.filter((x) => x !== l.id))}
				>
					{l.name}
				</Badge>
			{/each}
		</div>
	{/if}
	{#if attachments}
		<AttachmentList {att} />
	{/if}
	<EnvSecretsField bind:envRows invalid={envInvalid} />
</div>

<style>
	.addons {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.addon-row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-1);
	}
	.addon-labels {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
</style>
