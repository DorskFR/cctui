<script lang="ts">
	import ImageCompressionStatus from '$lib/components/molecules/ImageCompressionStatus.svelte';
	import type { Label } from '@bindings/Label';
	import { AutoGrid, Badge, Button, FileButton, Icon, Popover } from '@dorsk/tsumikit';
	import { labelTint, hueToColor } from '$lib/labels';
	import AttachmentList from '$lib/components/molecules/AttachmentList.svelte';
	import LabelMenu from '$lib/components/molecules/LabelMenu.svelte';
	import EnvSecretsField from './EnvSecretsField.svelte';
	import type { EnvRow } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		labelIds = $bindable(),
		envRows = $bindable(),
		pending = [],
		files,
		allLabels,
		envInvalid,
		attachments,
		labelActions,
		onfiles,
		onremovefile
	}: {
		labelIds: string[];
		envRows: EnvRow[];
		pending?: { file: File }[];
		files: File[];
		allLabels: Label[];
		envInvalid: boolean;
		attachments: boolean;
		labelActions: {
			createLabel: (name: string, color: string) => Promise<Label>;
			updateLabel: (id: string, patch: { name?: string; color?: string }) => Promise<Label>;
			deleteLabel: (id: string) => Promise<void>;
		};
		onfiles: (files: File[]) => void;
		onremovefile: (name: string) => void;
	} = $props();

	const selectedLabels = $derived(allLabels.filter((l) => labelIds.includes(l.id)));
	const attachedLabelIds = $derived(new Set(labelIds));
	function toggleLabel(l: Label) {
		labelIds = labelIds.includes(l.id) ? labelIds.filter((x) => x !== l.id) : [...labelIds, l.id];
	}
	async function createAndAttach(name: string) {
		if (!name.trim()) return;
		const label = await labelActions.createLabel(name, hueToColor(null));
		if (!labelIds.includes(label.id)) labelIds = [...labelIds, label.id];
	}

	// The kit keeps a popover's panel mounted after its first open, so LabelMenu's
	// mount-time autofocus only fires once; every reopen refocuses explicitly.
	let panel = $state<LabelMenu>();
	const addEnvRow = () => (envRows = [...envRows, { key: '', value: '' }]);
</script>

<div class="addons">
	<span class="addon-title">{m.spawn_optional_settings()}</span>
	<!-- Button labels never wrap, so the column floor must fit the longest localized
	     label plus icon ("Fichiers" / "Env vars"); short labels let three fit on a phone. -->
	<AutoGrid min="8rem" gap="var(--sp-2)" maxCols={3} align="stretch">
		<!-- The panel renders in the browser top layer, so it is neither clipped by
		     the Spawn Modal's <dialog> nor by its scrolling body. -->
		<Popover
			label={m.spawn_labels_aria()}
			placement="bottom-start"
			role="menu"
			haspopup="menu"
			block
			control
			style="gap: var(--sp-2)"
			panelStyle="max-height:calc(100dvh - 1rem);overflow-y:auto;overflow-x:hidden"
			onopen={() => panel?.focusSearch()}
		>
			{#snippet trigger()}
				<Icon name="tag" />{m.spawn_add_label()}
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
			<FileButton label={m.spawn_add_files()} icon="file-text" multiple {onfiles} />
		{/if}
		<Button block onclick={addEnvRow}><Icon name="plus" />{m.spawn_add_env_vars()}</Button>
	</AutoGrid>

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
		<ImageCompressionStatus {pending} />
		<AttachmentList {files} onremove={onremovefile} />
	{/if}
	<EnvSecretsField bind:envRows invalid={envInvalid} />
</div>

<style>
	.addons {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.addon-title {
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		color: var(--text-muted);
	}
	.addon-labels {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
</style>
