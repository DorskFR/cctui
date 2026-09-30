<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { EmptyState } from '@dorsk/tsumikit';
	import ConversationPane from '$lib/components/organisms/ConversationPane.svelte';
	import { tileLayout } from '$lib/tiles';
	import { m } from '$lib/paraglide/messages';

	let {
		sessions,
		onNavigate
	}: {
		sessions: SessionListItem[];
		onNavigate?: (id: string) => void;
	} = $props();

	let maximized = $state<string | null>(null);
	let width = $state(0);
	let height = $state(0);

	const shown = $derived(
		maximized && sessions.some((s) => s.id === maximized)
			? sessions.filter((s) => s.id === maximized)
			: sessions
	);
	const layout = $derived(tileLayout(shown.length, { width, height }));
</script>

<div
	class="tiles"
	data-journey="session-tiles"
	bind:clientWidth={width}
	bind:clientHeight={height}
	style:grid-template-columns="repeat({layout.tracks}, 1fr)"
	style:grid-template-rows="repeat({Math.max(layout.rows, 1)}, 1fr)"
>
	{#each shown as s, i (s.id)}
		{@const place = layout.placements[i]}
		<div
			class="tile"
			data-session-id={s.id}
			style:grid-column="{place?.start ?? 1} / span {place?.span ?? 1}"
			style:grid-row={place?.row ?? 1}
		>
			<ConversationPane
				chrome="tile"
				session={s}
				maximized={maximized === s.id}
				onmaximize={() => (maximized = maximized === s.id ? null : s.id)}
				{onNavigate}
			/>
		</div>
	{/each}
	{#if shown.length === 0}
		<div class="empty">
			<EmptyState icon="grid" title={m.tiles_empty_title()} description={m.tiles_empty_body()} />
		</div>
	{/if}
</div>

<style>
	/* The 1px gap IS the border: one hairline between neighbours instead of two
	   abutting ones, drawn by the grid's own background showing through. */
	.tiles {
		flex: 1;
		min-height: 0;
		display: grid;
		gap: 1px;
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
	.empty {
		grid-column: 1 / -1;
		grid-row: 1;
		display: grid;
		place-items: center;
		background: var(--bg);
	}
</style>
