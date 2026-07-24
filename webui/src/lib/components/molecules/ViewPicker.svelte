<script lang="ts">
	import { Icon, Select } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { VIEW_OPTIONS } from '../../../routes/sessions/sessions.logic';

	// One square control offering the 4 explicit layout × density combinations
	// (CCT-307). A native <select> overlaid transparently on a styled trigger
	// (the Select atom's `ghost` variant) gives the platform popup with zero
	// outside-click bookkeeping. `cardView` (list ⇄ card) and `dense` (compact ⇄
	// detailed) are bindable so the parent keeps owning persistence.
	let {
		cardView = $bindable(),
		dense = $bindable(),
		kanban = $bindable(),
		menu = false
	}: {
		cardView: boolean;
		dense: boolean;
		kanban: boolean;
		/** Render as a full-width labeled row for the overflow ⋯ menu. */
		menu?: boolean;
	} = $props();

	const mode = $derived(
		kanban ? 'kanban' : `${cardView ? 'card' : 'list'}-${dense ? 'compact' : 'detailed'}`
	);
	const label = $derived(VIEW_OPTIONS.find((o) => o.value === mode)?.label ?? m.sessions_view_label());

	function select(value: string) {
		const opt = VIEW_OPTIONS.find((o) => o.value === value);
		if (!opt) return;
		kanban = value === 'kanban';
		cardView = opt.card;
		dense = opt.dense;
	}
</script>

<div
	class="view-picker {menu ? 'menu-row' : 'btn-control btn-control-square'}"
	title={m.sessions_view_title({ view: label })}
	aria-label={m.sessions_view_title({ view: label })}
>
	<!-- Icons at size 18 to match the sibling IconButton controls (the old
	     unicode glyphs rendered at the inherited font size, so they read smaller).
	     `menu` for list; a raw layout-grid svg (no grid glyph in the registry) for
	     card. -->
	{#if cardView}
		<Icon size={18}>
			<rect x="3" y="3" width="7" height="7" rx="1" />
			<rect x="14" y="3" width="7" height="7" rx="1" />
			<rect x="3" y="14" width="7" height="7" rx="1" />
			<rect x="14" y="14" width="7" height="7" rx="1" />
		</Icon>
	{:else}
		<Icon name="menu" size={18} />
	{/if}
	<Select
		variant="ghost"
		aria-label={m.sessions_view_choose()}
		value={mode}
		onchange={(e) => select((e.currentTarget as HTMLSelectElement).value)}
	>
		{#each VIEW_OPTIONS as o (o.value)}
			<option value={o.value}>{o.label}</option>
		{/each}
	</Select>
</div>

<style>
	.view-picker {
		position: relative;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		white-space: nowrap;
		cursor: pointer;
	}
	/* Overflow-menu row: full-width, left-aligned icon + label (the aria-label,
	   e.g. "View: List · compact"). The ghost <Select> fills the row (inset:0), so
	   clicking anywhere opens the native picker. */
	.view-picker.menu-row {
		width: 100%;
		justify-content: flex-start;
		gap: var(--sp-2);
		min-height: 2.25rem;
		padding: var(--sp-1) var(--sp-2);
		border-radius: var(--r-sm);
		font-size: var(--fs-sm);
	}
	.view-picker.menu-row:hover {
		background: var(--bg-elevated-3, var(--bg-elevated-2));
	}
	.view-picker.menu-row::after {
		content: attr(aria-label);
		font-weight: var(--fw-medium);
	}
</style>
