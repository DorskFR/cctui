<script lang="ts">
	import { onMount } from 'svelte';
	import type { Label } from '@bindings/Label';
	import { Input } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import LabelList from './LabelList.svelte';
	import LabelEditModal from './LabelEditModal.svelte';

	// The shared label MENU PANEL — the contents, not a
	// trigger or popover shell — used by both the per-session picker (LabelBadge)
	// and the list-wide filter (LabelFilter): a filter/search Input over a
	// LabelList, plus the LabelEditModal so the pencil works wherever the menu
	// appears. Each caller keeps its own trigger + open/close
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
		autofocus = true,
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
		/** Focus the search box on mount. */
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

	let searchInput = $state<HTMLInputElement | null>(null);
	export function focusSearch() {
		searchInput?.focus({ preventScroll: true });
	}
	onMount(() => {
		if (autofocus) focusSearch();
	});

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
					? m.sessions_label_filter_create_n({ count: labels.length })
					: m.sessions_label_filter_create()
				: more
					? m.sessions_label_filter_n({ count: labels.length })
					: m.sessions_label_filter())
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

	// Prefer an exact match or the sole search result; otherwise keep creation.
	function onSubmit(e: SubmitEvent) {
		e.preventDefault();
		if (exactMatch) toggle(exactMatch);
		else if (query && matches.length === 1) toggle(matches[0]);
		else if (showCreate) create();
	}

	let editing = $state<Label | null>(null);
</script>

<form class="filter" onsubmit={onSubmit}>
	<!-- svelte-ignore a11y_autofocus -->
	<Input
		bind:el={searchInput}
		grow
		size="sm"
		placeholder={ph}
		aria-label={ph}
		{autofocus}
		bind:value={q}
		maxlength={40}
	/>
</form>

<LabelList
	labels={filtered}
	{selectedIds}
	{busy}
	createName={showCreate ? query : null}
	canCreate={!!onCreate}
	onToggle={toggle}
	onCreate={create}
	onEdit={onUpdate ? (l) => (editing = l) : undefined}
	{onClear}
/>

{#if editing}
	{#key editing}
		<LabelEditModal label={editing} {onUpdate} {onDelete} onclose={() => (editing = null)} />
	{/key}
{/if}

<style>
	/* The search box and the list share horizontal padding so each row's content
	   lines up flush with the input edges. A stable width keeps the panel from
	   jittering as rows filter in and out. */
	.filter {
		box-sizing: border-box;
		width: 15rem;
		display: flex;
		align-items: center;
		padding: var(--sp-1) var(--sp-2);
	}
</style>
