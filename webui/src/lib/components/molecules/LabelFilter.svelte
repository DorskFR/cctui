<script lang="ts">
	import type { Label } from '@bindings/Label';
	import { Badge, Icon, Popover } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import LabelMenu from './LabelMenu.svelte';

	// One square toolbar button that opens a popover of label toggles; a session
	// shows when it carries ANY selected label (OR semantics). The panel body is
	// the shared LabelMenu molecule; this wrapper owns the trigger and its count.
	// `selected` is bindable so the parent owns persistence. Renders nothing until
	// at least one label exists, so the caller doesn't have to guard.
	let {
		labels,
		selected = $bindable(),
		onUpdate,
		onDelete,
		menu = false
	}: {
		labels: Label[];
		selected: Set<string>;
		/** Render as a full-width labeled row for the overflow ⋯ menu. */
		menu?: boolean;
		// Editing the labels themselves (rename/recolor/delete) from the filter
		// menu — the same edit affordance the per-session picker has.
		onUpdate?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDelete?: (labelId: string) => void | Promise<void>;
	} = $props();

	const title = $derived(
		selected.size > 0
			? m.sessions_filtering_by_labels({ count: selected.size })
			: m.sessions_filter_by_label()
	);

	// The kit keeps a popover's panel mounted after its first open, so LabelMenu's
	// mount-time autofocus only fires once; every reopen refocuses explicitly.
	let panel = $state<ReturnType<typeof LabelMenu> | null>(null);

	function toggle(l: Label) {
		const next = new Set(selected);
		if (next.has(l.id)) next.delete(l.id);
		else next.add(l.id);
		selected = next;
	}
</script>

{#if labels.length > 0}
	<div class="label-filter" class:menu-row={menu}>
		<Popover
			label={m.sessions_filter_by_label()}
			placement="bottom-end"
			role="menu"
			haspopup="menu"
			{title}
			variant={menu ? 'ghost' : undefined}
			size={menu ? 'sm' : undefined}
			block={menu}
			tone={menu && selected.size > 0 ? 'accent' : 'none'}
			count={menu ? undefined : selected.size}
			style={menu ? 'justify-content:flex-start' : '--pop-box: var(--control-height)'}
			onopen={() => panel?.focusSearch()}
		>
			{#snippet trigger()}
				<Icon name="tag" size={18} />
				{#if menu}
					<span>{m.sessions_filter_by_label()}</span>
					{#if selected.size > 0}
						<span class="menu-count">
							<Badge size="xs" numeric tone="accent">{selected.size}</Badge>
						</span>
					{/if}
				{/if}
			{/snippet}
			<LabelMenu
				bind:this={panel}
				{labels}
				selectedIds={selected}
				cap={5}
				autofocus
				onToggle={toggle}
				onClear={() => (selected = new Set())}
				{onUpdate}
				{onDelete}
			/>
		</Popover>
	</div>
{/if}

<style>
	.label-filter {
		display: inline-flex;
		align-items: center;
		flex: none;
	}
	/* Overflow-menu row: full-width, left-aligned icon + label, matching the
	   drawer's ⋯ flyout rows. */
	.label-filter.menu-row {
		display: flex;
		width: 100%;
	}
	.menu-count {
		display: inline-flex;
		margin-inline-start: auto;
	}
</style>
