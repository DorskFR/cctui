<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import ConversationPane from '$lib/components/organisms/ConversationPane.svelte';
	import type { TileLayout } from '$lib/tiles';
	import type { TilesWorkspace } from '$lib/tiles.svelte';

	let {
		tiles,
		ids,
		layout,
		sessions,
		onnavigate
	}: {
		tiles: TilesWorkspace;
		/** The tiles actually on screen: every one, the maximised one, or the
		 *  focused one below the mobile breakpoint. */
		ids: string[];
		layout: TileLayout;
		/** Resolved rows for `ids`, in the same order; a tile whose row has not
		 *  landed yet is null and renders as a placeholder. */
		sessions: (SessionListItem | null)[];
		onnavigate: (from: string, to: string) => void;
	} = $props();

	function onkeydown(e: KeyboardEvent, id: string) {
		if (!e.altKey || e.ctrlKey || e.metaKey) return;
		if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
			e.preventDefault();
			tiles.move(id, -1);
		} else if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
			e.preventDefault();
			tiles.move(id, 1);
		}
	}
</script>

<div
	class="grid"
	data-journey="tiles"
	style:--tracks={layout.tracks}
	style:grid-template-columns="repeat({layout.tracks}, 1fr)"
	style:grid-template-rows="repeat({layout.rows}, 1fr)"
>
	{#each ids as id, i (id)}
		{@const place = layout.placements[i]}
		{@const s = sessions[i]}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="tile"
			class:focused={tiles.focused === id}
			data-journey="tile"
			data-session-id={id}
			style:grid-column="{place?.start ?? 1} / span {place?.span ?? 1}"
			style:grid-row={place?.row ?? 1}
			onfocusin={() => tiles.focus(id)}
			onkeydown={(e) => onkeydown(e, id)}
		>
			{#if s}
				<ConversationPane
					chrome="tile"
					session={s}
					maximized={tiles.maximized === id}
					onmaximize={() => tiles.toggleMaximize(id)}
					onclose={() => tiles.remove(id)}
					onNavigate={(to) => onnavigate(id, to)}
				/>
			{:else}
				<div class="pending">…</div>
			{/if}
		</div>
	{/each}
</div>

<style>
	/* The 1px gap IS the border: one hairline between neighbours instead of two
	   abutting ones, drawn by the grid's own background showing through. */
	.grid {
		flex: 1;
		min-height: 0;
		display: grid;
		gap: 1px;
		background: var(--border-strong);
		padding: 0;
	}
	.tile {
		position: relative;
		min-width: 0;
		min-height: 0;
		overflow: hidden;
		border-radius: 0;
		background: var(--bg);
	}
	.tile.focused {
		outline: 1px solid var(--accent);
		outline-offset: -1px;
	}
	.pending {
		display: grid;
		place-items: center;
		height: 100%;
		color: var(--text-muted);
	}
</style>
