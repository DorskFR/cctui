<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { IconButton } from '@dorsk/tsumikit';
	import { clickOutside } from '$lib/clickOutside';
	import LabelMenu from './LabelMenu.svelte';

	// One square toolbar button that opens a popover of label toggles; a session
	// shows when it carries ANY selected label (OR semantics, CCT-360). The menu
	// body is the shared LabelMenu molecule; this wrapper owns the IconButton
	// trigger, the count badge and the open/close (clickOutside). `selected` is
	// bindable so the parent owns persistence. Renders nothing until at least one
	// label exists, so the caller doesn't have to guard.
	let {
		labels,
		selected = $bindable(),
		onUpdate,
		onDelete
	}: {
		labels: Label[];
		selected: Set<string>;
		// Editing the labels themselves (rename/recolor/delete) from the filter
		// menu — the same edit affordance the per-session picker has.
		onUpdate?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDelete?: (labelId: string) => void | Promise<void>;
	} = $props();

	let open = $state(false);

	function toggle(l: Label) {
		const next = new Set(selected);
		if (next.has(l.id)) next.delete(l.id);
		else next.add(l.id);
		selected = next;
	}
</script>

{#if labels.length > 0}
	<div class="label-filter" use:clickOutside={() => (open = false)}>
		<IconButton
			variant="default"
			class="btn-control-square"
			icon="tag"
			label="Filter by label"
			title={selected.size > 0 ? `Filtering by ${selected.size} label(s)` : 'Filter by label'}
			aria-haspopup="true"
			aria-expanded={open}
			aria-pressed={selected.size > 0}
			onclick={() => (open = !open)}
		/>
		{#if selected.size > 0}<span class="count-badge" aria-hidden="true">{selected.size}</span>{/if}
		{#if open}
			<!-- svelte-ignore a11y_no_static_element_interactions -->
			<div
				class="menu"
				role="menu"
				aria-label="Labels"
				tabindex="-1"
				onkeydown={(e) => {
					if (e.key === 'Escape') open = false;
				}}
			>
				<LabelMenu
					{labels}
					selectedIds={selected}
					cap={5}
					autofocus
					onToggle={toggle}
					onClear={() => (selected = new Set())}
					{onUpdate}
					{onDelete}
				/>
			</div>
		{/if}
	</div>
{/if}

<style>
	.label-filter {
		position: relative;
		display: inline-flex;
		align-items: center;
		flex: none;
	}
	.count-badge {
		position: absolute;
		top: -0.35rem;
		right: -0.35rem;
		min-width: 1rem;
		height: 1rem;
		padding: 0 0.25rem;
		border-radius: 999px;
		background: var(--accent);
		color: var(--bg);
		font-size: 0.62rem;
		font-weight: var(--fw-semibold);
		line-height: 1rem;
		text-align: center;
		pointer-events: none;
	}
	.menu {
		position: absolute;
		top: calc(100% + var(--sp-1));
		right: 0;
		z-index: 40;
		display: flex;
		flex-direction: column;
		padding: var(--sp-1);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.4));
	}
</style>
