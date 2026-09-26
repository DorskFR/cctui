<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Badge, EmptyState, Icon } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { labelTint } from '$lib/labels';

	// The label rows of LabelMenu: the create row, one checkbox + hue-tinted chip
	// per label with an optional edit pencil, the empty state and the clear footer.
	let {
		labels,
		selectedIds,
		busy = false,
		createName = null,
		canCreate = false,
		onToggle,
		onCreate,
		onEdit,
		onClear
	}: {
		/** Rows to show, already filtered and capped. */
		labels: Label[];
		selectedIds: Set<string>;
		busy?: boolean;
		/** Name offered by the create row; null hides the row. */
		createName?: string | null;
		/** Whether the menu can create at all (picks the empty-state copy). */
		canCreate?: boolean;
		onToggle: (label: Label) => void;
		onCreate?: () => void;
		/** Open the edit modal for a label; omit to drop the per-row pencil. */
		onEdit?: (label: Label) => void;
		onClear?: () => void;
	} = $props();
</script>

<div class="list">
	<!-- Create sits right under the input, where the typed name is; in-list so it
	     shares the rows' width and never widens the panel. -->
	{#if createName}
		<button type="button" class="opt create" disabled={busy} onclick={onCreate}>
			<span class="check check-action" aria-hidden="true">+</span>
			<span class="create-label">{m.sessions_label_create()}</span>
			<Badge
				size="sm"
				truncate
				style="{labelTint({ name: createName, color: '' })};border-radius:var(--r-sm)"
			>
				<span class="opt-name">{createName}</span>
			</Badge>
		</button>
	{/if}

	{#each labels as l (l.id)}
		<div class="row">
			<button
				type="button"
				class="opt"
				aria-pressed={selectedIds.has(l.id)}
				disabled={busy}
				onclick={() => onToggle(l)}
			>
				<!-- The filter's checkbox: a solid-surfaced box that fills with accent +
				     a ✓ when checked (reads on any row, tinted or not). -->
				<span class="check" aria-hidden="true">{selectedIds.has(l.id) ? '✓' : ''}</span>
				<Badge size="sm" truncate style="{labelTint(l)};border-radius:var(--r-sm)">
					<span class="opt-name">{l.name}</span>
				</Badge>
			</button>
			{#if onEdit}
				<button
					type="button"
					class="edit"
					aria-label={m.sessions_label_edit_aria({ name: l.name })}
					disabled={busy}
					onclick={() => onEdit(l)}
				>
					<Icon name="edit" />
				</button>
			{/if}
		</div>
	{/each}

	{#if labels.length === 0 && !createName}
		<EmptyState
			size="inline"
			description={canCreate ? m.sessions_labels_empty_create() : m.sessions_labels_no_match()}
		/>
	{/if}

	{#if onClear && selectedIds.size > 0}
		<button type="button" class="opt clear" onclick={onClear}>
			<span class="check check-action" aria-hidden="true">✕</span>
			<span class="clear-label">{m.sessions_clear_filter()}</span>
		</button>
	{/if}
</div>

<style>
	/* Same stable width as LabelMenu's search box so the panel never jitters as
	   rows filter in and out. */
	.list {
		box-sizing: border-box;
		width: 15rem;
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
</style>
