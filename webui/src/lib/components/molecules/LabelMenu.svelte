<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Badge, Button, Field, Icon, Input, Modal } from '@dorsk/tsumikit';
	import Swatch from '$lib/components/atoms/Swatch.svelte';
	import { LABEL_HUES, labelTint, storedHue, hueToColor } from '$lib/labels';

	// The shared label MENU PANEL (CCT-360 convergence) — the contents, not a
	// trigger or popover shell. Both the per-session picker (LabelBadge) and the
	// list-wide filter (LabelFilter) kept hand-rolling the same panel: a
	// filter/search Input on top and a column of label rows beneath. This molecule
	// owns that panel AND the rename/recolor/delete edit Modal, so the pencil works
	// wherever the menu appears. Each caller keeps its own trigger + open/close
	// (LabelFilter's IconButton + clickOutside, LabelBadge's Popover).
	//
	// A row is the filter's checkbox + the picker's hue-tinted Badge chip. The
	// per-row edit pencil shows when `onUpdate` is given; "Create" when `onCreate`
	// is; the "Clear" footer when `onClear` is.
	let {
		labels,
		selectedIds,
		cap = 8,
		placeholder,
		autofocus = false,
		busy = false,
		onToggle,
		onCreate,
		onUpdate,
		onDelete,
		onClear
	}: {
		/** All selectable labels, in recency order (most recent first). */
		labels: Label[];
		/** Ids currently checked — attached labels, or the active filter set. */
		selectedIds: Set<string>;
		/** Rows shown with no active query; the rest reachable by searching. */
		cap?: number;
		/** Search box placeholder; defaults to reflect whether create is offered. */
		placeholder?: string;
		/** Focus the search box on mount (the filter's manual menu wants this). */
		autofocus?: boolean;
		/** Disable row interactions while a mutation is in flight. */
		busy?: boolean;
		/** Toggle a label (attach/detach, or add/remove from the filter). */
		onToggle: (label: Label) => void;
		/** Create-and-select the typed name; omit to drop the create affordance. */
		onCreate?: (name: string) => void;
		/** Rename/recolor a label; omit to drop the per-row edit pencil. */
		onUpdate?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		/** Delete a label from the edit modal; omit to drop the Delete button. */
		onDelete?: (labelId: string) => void | Promise<void>;
		/** Clear-all footer; omit to drop it (picker mode). */
		onClear?: () => void;
	} = $props();

	let q = $state('');
	const query = $derived(q.trim());
	// Whether the cap is hiding labels — when it is, the placeholder names the
	// total count so it's clear there's more to reach by typing (the only place
	// that "more" hint lives). Otherwise it just reflects whether this menu can
	// also create (only the picker can).
	const more = $derived(labels.length > cap);
	const ph = $derived(
		placeholder ??
			(onCreate
				? more
					? `Filter ${labels.length} labels or create…`
					: 'Filter or create…'
				: more
					? `Filter ${labels.length} labels…`
					: 'Filter labels…')
	);
	// Capped in BOTH states — searching must not blow the list past `cap`.
	const matches = $derived(
		query ? labels.filter((l) => l.name.toLowerCase().includes(query.toLowerCase())) : labels
	);
	const filtered = $derived(matches.slice(0, cap));
	const exactMatch = $derived(
		labels.find((l) => l.name.toLowerCase() === query.toLowerCase()) ?? null
	);
	const showCreate = $derived(!!onCreate && !!query && !exactMatch);

	function toggle(l: Label) {
		if (!busy) onToggle(l);
	}

	function create() {
		if (!query || busy || !onCreate) return;
		onCreate(query);
		q = '';
	}

	// Enter in the search box: toggle the single exact match, else create.
	function onSubmit(e: SubmitEvent) {
		e.preventDefault();
		if (exactMatch) toggle(exactMatch);
		else if (showCreate) create();
	}

	// --- Edit modal (rename + recolor + delete), keyed on id. ---
	let editing = $state<Label | null>(null);
	let editName = $state('');
	let editHue = $state<number | null>(null);
	let editBusy = $state(false);
	let editError = $state('');

	function openEdit(l: Label) {
		editing = l;
		editName = l.name;
		editHue = storedHue(l.color);
		editError = '';
	}

	function closeEdit() {
		editing = null;
		editError = '';
	}

	async function saveEdit() {
		if (!editing || !onUpdate || editBusy) return;
		const name = editName.trim();
		if (!name) {
			editError = 'Name is required.';
			return;
		}
		editBusy = true;
		editError = '';
		try {
			const patch: { name?: string; color?: string } = { color: hueToColor(editHue) };
			if (name !== editing.name) patch.name = name;
			await onUpdate(editing.id, patch);
			closeEdit();
		} catch (e) {
			editError = e instanceof Error ? e.message : 'Could not save the label.';
		} finally {
			editBusy = false;
		}
	}

	async function deleteEditing() {
		if (!editing || !onDelete || editBusy) return;
		editBusy = true;
		editError = '';
		try {
			await onDelete(editing.id);
			closeEdit();
		} catch (e) {
			editError = e instanceof Error ? e.message : 'Could not delete the label.';
		} finally {
			editBusy = false;
		}
	}
</script>

<form class="filter" onsubmit={onSubmit}>
	<!-- svelte-ignore a11y_autofocus -->
	<Input size="sm" placeholder={ph} {autofocus} bind:value={q} maxlength={40} />
</form>

<div class="list">
	<!-- Create sits right under the input, where the typed name is; in-list so it
	     shares the rows' width and never widens the panel. -->
	{#if showCreate}
		<button type="button" class="opt create" disabled={busy} onclick={create}>
			<span class="check check-action" aria-hidden="true">+</span>
			<span class="create-label">Create</span>
			<Badge
				size="sm"
				class="label"
				style="{labelTint({ name: query, color: '' })};border-radius:var(--r-sm)"
			>
				<span class="opt-name">{query}</span>
			</Badge>
		</button>
	{/if}

	{#each filtered as l (l.id)}
		<div class="row">
			<button
				type="button"
				class="opt"
				aria-pressed={selectedIds.has(l.id)}
				disabled={busy}
				onclick={() => toggle(l)}
			>
				<!-- The filter's checkbox: a solid-surfaced box that fills with accent +
				     a ✓ when checked (reads on any row, tinted or not). -->
				<span class="check" aria-hidden="true">{selectedIds.has(l.id) ? '✓' : ''}</span>
				<Badge size="sm" class="label" style="{labelTint(l)};border-radius:var(--r-sm)">
					<span class="opt-name">{l.name}</span>
				</Badge>
			</button>
			{#if onUpdate}
				<button
					type="button"
					class="edit"
					aria-label={`Edit ${l.name}`}
					disabled={busy}
					onclick={() => openEdit(l)}
				>
					<Icon name="edit" />
				</button>
			{/if}
		</div>
	{/each}

	{#if filtered.length === 0 && !showCreate}
		<p class="empty">
			{onCreate ? 'No labels yet — type to create one.' : 'No matching labels'}
		</p>
	{/if}

	{#if onClear && selectedIds.size > 0}
		<button type="button" class="opt clear" onclick={onClear}>
			<span class="check check-action" aria-hidden="true">✕</span>
			<span class="clear-label">Clear filter</span>
		</button>
	{/if}
</div>

<!-- Edit modal: rename + recolor (+ delete) a single label, keyed on id. The
     propagation guard keeps a backdrop/close click from bubbling to a clickable
     ancestor (e.g. the session Card the picker lives on). -->
{#if editing}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<span
		class="modal-host"
		onpointerdown={(e) => e.stopPropagation()}
		onclick={(e) => e.stopPropagation()}
		onkeydown={(e) => e.stopPropagation()}
	>
		<Modal title="Edit label" size="sm" onclose={closeEdit}>
			{#snippet body()}
				<div class="edit-form">
					<Field label="Name">
						<Input
							bind:value={editName}
							maxlength={40}
							placeholder="Label name"
							onkeydown={(e: KeyboardEvent) => {
								if (e.key === 'Enter') {
									e.preventDefault();
									saveEdit();
								}
							}}
						/>
					</Field>
					<Field label="Color">
						<div class="hues" role="radiogroup" aria-label="Label color">
							<Swatch
								hue={null}
								active={editHue == null}
								title="Auto (name hash)"
								aria-label="Auto color"
								onclick={() => (editHue = null)}>A</Swatch
							>
							{#each LABEL_HUES as h (h)}
								<Swatch
									hue={h}
									active={editHue === h}
									title={`Hue ${h}`}
									aria-label={`Hue ${h}`}
									onclick={() => (editHue = h)}
								/>
							{/each}
						</div>
					</Field>
					<div class="preview-row">
						<span class="preview-label">Preview</span>
						<Badge
							class="label"
							style="{labelTint({ name: editName || 'label', color: hueToColor(editHue) })};border-radius:var(--r-sm)"
						>
							<span class="opt-name">{editName || 'label'}</span>
						</Badge>
					</div>
					{#if editError}<p class="edit-error">{editError}</p>{/if}
				</div>
			{/snippet}
			{#snippet footer()}
				{#if onDelete}
					<Button variant="danger" disabled={editBusy} onclick={deleteEditing}>Delete</Button>
				{/if}
				<Button variant="primary" block disabled={editBusy || !editName.trim()} onclick={saveEdit}>
					{#if editBusy}<span class="spin"></span>{:else}Save{/if}
				</Button>
			{/snippet}
		</Modal>
	</span>
{/if}

<style>
	/* The search box and the list share horizontal padding so each row's content
	   lines up flush with the input edges. A stable width keeps the panel from
	   jittering as rows filter in and out. */
	.filter,
	.list {
		box-sizing: border-box;
		width: 15rem;
	}
	.filter {
		display: flex;
		align-items: center;
		padding: var(--sp-1) var(--sp-2);
	}
	.filter :global(input) {
		flex: 1;
		min-width: 0;
	}

	.list {
		display: flex;
		flex-direction: column;
		gap: 0.05rem;
		max-height: 14rem;
		overflow-y: auto;
		/* Top padding both spaces the list from the input and keeps the scroll
		   container from clipping the first row's focus outline. */
		padding: var(--sp-2) var(--sp-2) var(--sp-1);
	}
	.row {
		display: flex;
		align-items: stretch;
		gap: var(--sp-1);
	}
	/* Rows match the filter Input's height so the menu reads as an even stack. */
	.opt,
	.edit {
		min-height: 2rem;
	}
	.opt {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex: 1;
		min-width: 0;
		padding: var(--sp-1) var(--sp-2);
		border: none;
		background: none;
		color: var(--text);
		cursor: pointer;
		border-radius: var(--r-sm);
		font-size: var(--fs-sm);
		text-align: left;
	}
	.opt:hover:not(:disabled),
	.edit:hover:not(:disabled) {
		background: var(--bg-elevated-2);
	}
	/* The filter's checkbox: a SOLID-surfaced box (a transparent one would vanish
	   over a tint), neutral fill + strong border, going accent fill + ✓ when
	   checked. */
	.check {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.05rem;
		height: 1.05rem;
		flex: none;
		border-radius: var(--r-sm);
		border: 1.5px solid var(--border-strong);
		background: var(--bg-elevated);
		color: var(--text);
		font-size: 0.75rem;
		line-height: 1;
	}
	.opt[aria-pressed='true'] .check {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--bg);
	}
	/* The create/clear rows aren't checkable — their box is just a glyph holder. */
	.check-action {
		color: var(--text-muted);
	}
	/* The menu row shows the real (tinted) label Badge — same chip the card
	   renders. Let it shrink so long names ellipsis inside the fixed-width panel. */
	.opt :global(.label) {
		min-width: 0;
		overflow: hidden;
	}
	.create {
		color: var(--text-muted);
	}
	.create-label {
		flex: none;
		color: var(--text-muted);
		font-size: var(--fs-sm);
	}
	.opt-name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.clear {
		color: var(--text-muted);
	}
	.clear-label {
		flex: 1 1 auto;
	}
	.edit {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		width: 2rem;
		padding: var(--sp-1);
		border: none;
		background: none;
		color: var(--text-muted);
		cursor: pointer;
		border-radius: var(--r-sm);
	}
	.empty {
		margin: 0;
		padding: var(--sp-2);
		color: var(--text-muted);
		font-size: var(--fs-sm);
	}

	/* No box of its own — purely an event boundary so the dialog's clicks don't
	   bubble to a clickable ancestor. Events still follow the DOM tree under
	   display:contents. */
	.modal-host {
		display: contents;
	}
	/* Edit modal body. */
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
	.edit-error {
		margin: 0;
		color: var(--danger, var(--text));
		font-size: var(--fs-sm);
	}
	.spin {
		display: inline-block;
		width: 0.9em;
		height: 0.9em;
		border: 2px solid currentColor;
		border-right-color: transparent;
		border-radius: 50%;
		animation: spin 0.6s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
</style>
