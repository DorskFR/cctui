<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Badge, Button, Field, Icon, Input, Modal, Popover } from '@dorsk/tsumikit';
	import Swatch from '$lib/components/atoms/Swatch.svelte';
	import { LABEL_HUES, labelTint, storedHue, hueToColor } from '$lib/labels';

	// Session-label strip + picker (CCT-360, reworked CCT-? onto a single
	// filter-and-pick popover). Attached labels render as hue-tinted, removable
	// Badges. When `editable`, a `tag` trigger opens ONE popover that is a combined
	// filter/create box: a text input at the top filters the existing labels and,
	// when nothing matches, becomes a "Create" affordance directly beneath it. Each
	// row toggles attach/detach; the pencil opens a proper edit Modal where the
	// label can be renamed AND recolored (or deleted) — the old inline hue strip
	// could only recolor.
	//
	// Recolor/rename go through PATCH /labels/{id} (`onUpdate`), keyed on id so a
	// rename never orphans the old name. Create still uses POST /labels.
	let {
		labels,
		editable = false,
		allLabels = [],
		onCreate,
		onAttach,
		onDetach,
		onUpdate,
		onDelete
	}: {
		labels: Label[];
		editable?: boolean;
		allLabels?: Label[];
		onCreate?: (name: string, color: string) => Promise<Label>;
		onAttach?: (labelId: string) => void | Promise<void>;
		onDetach?: (labelId: string) => void | Promise<void>;
		onUpdate?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDelete?: (labelId: string) => void | Promise<void>;
	} = $props();

	// How many labels to show in the picker when not actively filtering. The list
	// arrives most-recently-used-or-created first from the server, so this surfaces
	// the handful you actually reach for instead of the entire history. Typing in
	// the filter searches across ALL labels regardless of this cap.
	const MAX_VISIBLE = 8;

	let q = $state('');
	let busy = $state(false);

	// The label being edited in the modal (null = closed), plus its draft fields.
	let editing = $state<Label | null>(null);
	let editName = $state('');
	let editHue = $state<number | null>(null);
	let editBusy = $state(false);
	let editError = $state('');

	const attachedIds = $derived(new Set(labels.map((l) => l.id)));
	const query = $derived(q.trim());
	const filtered = $derived(
		query
			? allLabels.filter((l) => l.name.toLowerCase().includes(query.toLowerCase()))
			: allLabels.slice(0, MAX_VISIBLE)
	);
	const exactMatch = $derived(
		allLabels.find((l) => l.name.toLowerCase() === query.toLowerCase()) ?? null
	);
	const showCreate = $derived(!!query && !exactMatch);
	// Whether the cap is hiding labels (only relevant with no active filter).
	const hiddenCount = $derived(query ? 0 : Math.max(0, allLabels.length - MAX_VISIBLE));

	async function toggleExisting(l: Label) {
		if (busy) return;
		busy = true;
		try {
			if (attachedIds.has(l.id)) await onDetach?.(l.id);
			else await onAttach?.(l.id);
		} finally {
			busy = false;
		}
	}

	async function createAndAttach() {
		if (!query || busy || !onCreate) return;
		busy = true;
		try {
			const label = await onCreate(query, hueToColor(null));
			if (!attachedIds.has(label.id)) await onAttach?.(label.id);
			q = '';
		} finally {
			busy = false;
		}
	}

	// Enter in the filter box: toggle the single exact match, else create.
	function onSubmit(e: SubmitEvent) {
		e.preventDefault();
		if (exactMatch) toggleExisting(exactMatch);
		else createAndAttach();
	}

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

{#if labels.length > 0 || editable}
	<!-- Swallow clicks/keys so interacting with labels never taps the parent
	     session card (the row is a clickable Card). svelte-ignore: the span is a
	     pure event boundary, not itself an interactive control. -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<span
		class="labels"
		onpointerdown={(e) => e.stopPropagation()}
		onclick={(e) => e.stopPropagation()}
		onkeydown={(e) => e.stopPropagation()}
	>
		{#each labels as l (l.id)}
			<Badge
				class="label"
				style="{labelTint(l)};border-radius:var(--r-sm)"
				removable={editable}
				onremove={() => {
					if (!busy) onDetach?.(l.id);
				}}
			>
				<span class="name">{l.name}</span>
			</Badge>
		{/each}

		{#if editable}
			<span class="add">
				<Popover
					label="Edit labels"
					placement="bottom-start"
					triggerClass="tag-trigger"
					onclose={() => {
						q = '';
					}}
				>
					{#snippet trigger()}<Icon name="tag" />{/snippet}

					<form class="filter" onsubmit={onSubmit}>
						<Input size="sm" placeholder="Filter or create…" bind:value={q} maxlength={40} />
					</form>

					<!-- Create affordance sits directly under the input (where the typed
					     name is), not at the bottom of the list. -->
					{#if showCreate}
						<button type="button" class="opt create" disabled={busy} onclick={createAndAttach}>
							<span class="check" aria-hidden="true">+</span>
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

					<div class="list">
						{#each filtered as l (l.id)}
							<div class="row">
								<button
									type="button"
									class="opt"
									aria-pressed={attachedIds.has(l.id)}
									disabled={busy}
									onclick={() => toggleExisting(l)}
								>
									<span class="check">
										{#if attachedIds.has(l.id)}<Icon name="check" />{/if}
									</span>
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

						{#if hiddenCount > 0}
							<p class="more">Type to search {hiddenCount} more…</p>
						{:else if filtered.length === 0 && !showCreate}
							<p class="empty">No labels yet — type to create one.</p>
						{/if}
					</div>
				</Popover>
			</span>
		{/if}
	</span>
{/if}

<!-- Edit modal: rename + recolor (+ delete) a single label, keyed on id. The
     native <dialog> stays in THIS subtree, so without swallowing pointer/click
     events here a backdrop/close click bubbles up to the clickable session Card
     and opens the conversation. -->
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
						<span class="name">{editName || 'label'}</span>
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
	.labels {
		display: inline-flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-1);
	}
	/* No box of its own — purely an event boundary so the dialog's clicks don't
	   bubble to the card. Events still follow the DOM tree under display:contents. */
	.modal-host {
		display: contents;
	}
	.name {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	/* `tag` trigger: a small ghost icon-button reading clearly as "labels". */
	.add :global(.tag-trigger) {
		min-width: 1.5rem;
		min-height: 1.5rem;
		padding: var(--sp-1);
		color: var(--text-muted);
	}
	.add :global(.tag-trigger:hover) {
		color: var(--text);
		background: var(--bg-elevated-2);
	}

	/* The panel keeps a stable width. The filter box and the list share the same
	   horizontal padding so each row's content lines up flush with the input edges
	   (the edit button no longer juts past the input). */
	.filter,
	.list,
	.create {
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
		padding: 0 var(--sp-2) var(--sp-1);
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
	.check {
		display: inline-flex;
		align-items: center;
		width: 0.9rem;
		flex: none;
		color: var(--accent);
	}
	/* The menu row shows the real (tinted) label Badge — same chip the card
	   renders — rather than a separate dot + plain text. Let it shrink so long
	   names ellipsis inside the fixed-width panel. */
	.opt :global(.label) {
		min-width: 0;
		overflow: hidden;
	}
	.create {
		padding: var(--sp-1) var(--sp-2);
		margin: 0 var(--sp-2);
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
	.empty,
	.more {
		margin: 0;
		padding: var(--sp-2);
		color: var(--text-muted);
		font-size: var(--fs-sm);
	}

	/* Edit modal body. */
	.edit-form {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
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
