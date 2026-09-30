<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Button, EmptyState, Popover, Text } from '@dorsk/tsumikit';
	import ConversationPane from '$lib/components/organisms/ConversationPane.svelte';
	import { paneCapacity, tileLayout } from '$lib/tiles';
	import { m } from '$lib/paraglide/messages';

	let {
		sessions,
		onNavigate,
		onOpen
	}: {
		sessions: SessionListItem[];
		onNavigate?: (id: string) => void;
		/** Open a session the grid had no room for, in the normal drawer. */
		onOpen?: (s: SessionListItem) => void;
	} = $props();

	let maximized = $state<string | null>(null);
	let width = $state(0);
	let height = $state(0);

	const shown = $derived(
		maximized && sessions.some((s) => s.id === maximized)
			? sessions.filter((s) => s.id === maximized)
			: sessions
	);
	// 0 until the area has been measured, so no pane mounts — and no history is
	// fetched — for a tile of unknown size.
	const capacity = $derived(paneCapacity({ width, height }));
	const panes = $derived(shown.slice(0, capacity));
	const overflow = $derived(shown.slice(capacity));
	const layout = $derived(tileLayout(panes.length, { width, height }));
</script>

{#if overflow.length}
	<div class="more">
		<Popover label={m.tiles_more({ count: overflow.length })} variant="default" size="sm">
			{#snippet trigger()}{m.tiles_more({ count: overflow.length })}{/snippet}
			<div class="more-list">
				<Text size="xs" tone="muted">{m.tiles_more_help()}</Text>
				{#each overflow as s (s.id)}
					<Button
						size="sm"
						variant="ghost"
						block
						style="justify-content:flex-start"
						onclick={() => onOpen?.(s)}
					>
						{s.name || s.working_dir}
					</Button>
				{/each}
			</div>
		</Popover>
	</div>
{/if}

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
	{#if sessions.length === 0}
		<div class="empty">
			<EmptyState icon="grid" title={m.tiles_empty_title()} description={m.tiles_empty_body()} />
		</div>
	{/if}
</div>

<style>
	.more {
		flex: none;
		display: flex;
		justify-content: flex-end;
		padding: 0 var(--sp-2) var(--sp-1);
	}
	.more-list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		max-height: 50vh;
		overflow-y: auto;
		min-width: 16rem;
	}
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
	.empty {
		grid-column: 1 / -1;
		grid-row: 1;
		display: grid;
		place-items: center;
		background: var(--bg);
	}
</style>
