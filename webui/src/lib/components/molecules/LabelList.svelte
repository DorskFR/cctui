<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Badge, Button, EmptyState, IconButton } from '@dorsk/tsumikit';
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

	// Left-aligned list-row geometry over the kit Button's ghost chrome; `style`
	// is the only hook that reaches the element Button renders.
	const ROW = 'justify-content:flex-start;text-align:left;min-width:0;font-size:var(--fs-sm)';
</script>

<div class="list">
	<!-- Create sits right under the input, where the typed name is; in-list so it
	     shares the rows' width and never widens the panel. -->
	{#if createName}
		<Button variant="ghost" size="sm" block class="create" style={ROW} disabled={busy} onclick={onCreate}>
			<span class="check check-action" aria-hidden="true">+</span>
			<span class="create-label">{m.sessions_label_create()}</span>
			<Badge
				size="sm"
				truncate
				style="{labelTint({ name: createName, color: '' })};border-radius:var(--r-sm)"
			>
				<span class="opt-name">{createName}</span>
			</Badge>
		</Button>
	{/if}

	{#each labels as l (l.id)}
		<div class="row">
			<Button
				variant="ghost"
				size="sm"
				grow
				style={ROW}
				aria-pressed={selectedIds.has(l.id)}
				disabled={busy}
				onclick={() => onToggle(l)}
			>
				<!-- The filter's checkbox: a solid-surfaced box that fills with accent +
				     a ✓ when checked (reads on any row, tinted or not). -->
				<span class="check" class:on={selectedIds.has(l.id)} aria-hidden="true">{selectedIds.has(l.id) ? '✓' : ''}</span>
				<Badge size="sm" truncate style="{labelTint(l)};border-radius:var(--r-sm)">
					<span class="opt-name">{l.name}</span>
				</Badge>
			</Button>
			{#if onEdit}
				<IconButton
					icon="edit"
					variant="ghost"
					box="sm"
					label={m.sessions_label_edit_aria({ name: l.name })}
					disabled={busy}
					onclick={() => onEdit(l)}
				/>
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
		<Button variant="ghost" size="sm" block style={ROW} onclick={onClear}>
			<span class="check check-action" aria-hidden="true">✕</span>
			<span class="clear-label">{m.sessions_clear_filter()}</span>
		</Button>
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
	.check.on {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--bg);
	}
	/* The create/clear rows aren't checkable — their box is just a glyph holder. */
	.check-action {
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
	.clear-label {
		flex: 1 1 auto;
		color: var(--text-muted);
	}
</style>
