<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { EmptyState } from '@dorsk/tsumikit';
	import ConversationPane from '$lib/components/organisms/ConversationPane.svelte';
	import { fittingPaneCount, tileLayout } from '$lib/tiles';
	import { m } from '$lib/paraglide/messages';

	let {
		sessions,
		onNavigate,
		onoverflow
	}: {
		sessions: SessionListItem[];
		onNavigate?: (id: string) => void;
		/** The sessions the grid had no room for. Reported rather than rendered:
		 *  a chip above the grid would eat the height the capacity is measured
		 *  from, so the toolbar shows it instead. */
		onoverflow?: (items: SessionListItem[]) => void;
	} = $props();

	let maximized = $state<string | null>(null);
	let picked = $state<string | null>(null);
	let width = $state(0);
	let height = $state(0);

	const shown = $derived(
		maximized && sessions.some((s) => s.id === maximized)
			? sessions.filter((s) => s.id === maximized)
			: sessions
	);
	// 0 until the area has been measured, so no pane mounts — and no history is
	// fetched — for a tile of unknown size.
	const capacity = $derived(fittingPaneCount(shown.length, { width, height }));
	const panes = $derived(shown.slice(0, capacity));
	const overflow = $derived(shown.slice(capacity));
	const layout = $derived(tileLayout(panes.length, { width, height }));
	// Exactly one tile holds the keyboard at a time: the one last clicked or
	// focused into, else the first, so Escape can never hit every session.
	const active = $derived(
		picked && panes.some((p) => p.id === picked) ? picked : (panes[0]?.id ?? null)
	);

	$effect(() => {
		onoverflow?.(overflow);
	});
</script>

<div
	class="tiles"
	data-journey="session-tiles"
	bind:clientWidth={width}
	bind:clientHeight={height}
	style:grid-template-columns="repeat({layout.tracks}, minmax(0, 1fr))"
	style:grid-template-rows="repeat({Math.max(layout.rows, 1)}, minmax(0, 1fr))"
>
	{#each panes as s, i (s.id)}
		{@const place = layout.placements[i]}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="tile"
			class:active={active === s.id}
			data-session-id={s.id}
			data-active={active === s.id ? 'on' : undefined}
			style:grid-column="{place?.start ?? 1} / span {place?.span ?? 1}"
			style:grid-row={place?.row ?? 1}
			onpointerdown={() => (picked = s.id)}
			onfocusin={() => (picked = s.id)}
		>
			<ConversationPane
				chrome="tile"
				active={active === s.id}
				session={s}
				maximized={maximized === s.id}
				onmaximize={() => (maximized = maximized === s.id ? null : s.id)}
				{onNavigate}
			/>
		</div>
	{/each}
	{#if sessions.length === 0}
		<div class="empty">
			<EmptyState icon="grid" title={m.tiles_empty_title()} description={m.tiles_empty_body()} />
		</div>
	{/if}
</div>

<style>
	/* The 1px gap IS the border: one hairline between neighbours instead of two
	   abutting ones, drawn by the grid's own background showing through. */
	/* minmax(0, …) on both axes, not `1fr`: a bare `1fr` floors at the pane's
	   min-content height, so the rows refuse to shrink and the page grows a
	   scrollbar instead of the transcripts scrolling inside their tiles. */
	.tiles {
		flex: 1;
		min-height: 0;
		display: grid;
		gap: 1px;
		overflow: hidden;
		background: var(--border-strong);
	}
	.tile {
		position: relative;
		min-width: 0;
		min-height: 0;
		overflow: hidden;
		border-radius: 0;
		background: var(--bg);
	}
	/* Inset so the ring reads as the tile's own edge inside the 1px grid gap. */
	.tile.active {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
	}
	.empty {
		grid-column: 1 / -1;
		grid-row: 1;
		display: grid;
		place-items: center;
		background: var(--bg);
	}
</style>
