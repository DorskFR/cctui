<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { AutoGrid, Badge, Button, FileButton, Icon } from '@dorsk/tsumikit';
	import { clickOutside } from '$lib/clickOutside';
	import { labelTint, hueToColor } from '$lib/labels';
	import AttachmentList from '$lib/components/molecules/AttachmentList.svelte';
	import LabelMenu from '$lib/components/molecules/LabelMenu.svelte';
	import EnvSecretsField from './EnvSecretsField.svelte';
	import type { EnvRow } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		labelIds = $bindable(),
		envRows = $bindable(),
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

	// The panel uses the native popover API so it renders in the top layer,
	// above the Modal's <dialog> and outside its scrolling body; placed from the
	// trigger rect, flipped above when it would overflow the viewport.
	let menuOpen = $state(false);
	let triggerEl = $state<HTMLElement | null>(null);
	let menuEl = $state<HTMLElement | null>(null);
	let menuPos = $state({ top: 0, left: 0 });
	function openMenu() {
		if (!triggerEl) return;
		const r = triggerEl.getBoundingClientRect();
		menuPos = { top: r.bottom + 4, left: r.left };
		menuOpen = true;
		menuEl?.showPopover();
		requestAnimationFrame(placeMenu);
	}
	function placeMenu() {
		if (!triggerEl || !menuEl) return;
		const gap = 4;
		const t = triggerEl.getBoundingClientRect();
		const p = menuEl.getBoundingClientRect();
		const spaceBelow = window.innerHeight - t.bottom;
		const flipUp = spaceBelow < p.height + gap && t.top > spaceBelow;
		const top = flipUp ? Math.max(gap, t.top - p.height - gap) : t.bottom + gap;
		const left = Math.max(gap, Math.min(t.left, window.innerWidth - p.width - gap));
		menuPos = { top, left };
	}
	function closeMenu() {
		if (!menuOpen) return;
		menuOpen = false;
		menuEl?.hidePopover();
	}
	const toggleMenu = () => (menuOpen ? closeMenu() : openMenu());
	const addEnvRow = () => (envRows = [...envRows, { key: '', value: '' }]);
</script>

<div class="addons">
	<span class="addon-title">{m.spawn_optional_settings()}</span>
	<!-- Button labels never wrap, so the column floor must fit the longest localized
	     label plus icon ("Ajouter des fichiers" in fr); 8rem let them overflow. -->
	<AutoGrid min="12rem" gap="var(--sp-2)" maxCols={3} align="stretch">
		<div class="label-add" bind:this={triggerEl} use:clickOutside={closeMenu}>
			<Button block aria-haspopup="true" aria-expanded={menuOpen} onclick={toggleMenu}>
				<Icon name="tag" />{m.spawn_add_label()}
			</Button>
			<div
				bind:this={menuEl}
				class="label-menu"
				popover="manual"
				role="menu"
				aria-label={m.spawn_labels_aria()}
				tabindex="-1"
				style:top="{menuPos.top}px"
				style:left="{menuPos.left}px"
				onkeydown={(e) => {
					if (e.key === 'Escape') closeMenu();
				}}
			>
				{#if menuOpen}
					<LabelMenu
						labels={allLabels}
						selectedIds={attachedLabelIds}
						cap={5}
						autofocus
						onToggle={toggleLabel}
						onCreate={createAndAttach}
						onUpdate={(labelId, patch) => labelActions.updateLabel(labelId, patch)}
						onDelete={(labelId) => labelActions.deleteLabel(labelId)}
					/>
				{/if}
			</div>
		</div>
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
	.label-add {
		display: flex;
		align-items: stretch;
	}
	.label-menu {
		position: fixed;
		inset: auto;
		margin: 0;
		padding: var(--sp-1);
		display: flex;
		flex-direction: column;
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.4));
	}
	.label-menu:not(:popover-open) {
		display: none;
	}
</style>
