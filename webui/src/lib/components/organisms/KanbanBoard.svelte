<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	type Column = { key: string; label: string; sessions: SessionListItem[] };
	let { columns, card }: { columns: Column[]; card: Snippet<[SessionListItem]> } = $props();
</script>

<div class="board">
	{#each columns as col (col.key)}
		<section class="column" data-col={col.key}>
			<div class="col-header">
				{col.label} <span class="count">{col.sessions.length}</span>
			</div>
			<div class="col-body">
				{#if col.sessions.length === 0}
					<div class="col-empty"><Text size="sm" tone="muted">{m.sessions_kanban_empty()}</Text></div>
				{:else}
					{#each col.sessions as s (s.id)}
						{@render card(s)}
					{/each}
				{/if}
			</div>
		</section>
	{/each}
</div>

<style>
	.board {
		display: grid;
		grid-auto-flow: column;
		grid-auto-columns: minmax(17.5rem, 1fr);
		gap: var(--sp-3);
		overflow-x: auto;
		scroll-snap-type: x proximity;
		padding-bottom: var(--sp-2);
	}
	.column {
		scroll-snap-align: start;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}
	.col-header {
		position: sticky;
		top: 0;
		z-index: 1;
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-1);
		font-size: var(--fs-sm);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--text-muted);
		background: var(--bg);
	}
	.col-header .count {
		font-weight: 400;
		opacity: 0.7;
	}
	.column[data-col='blocked'] .col-header {
		color: var(--warn);
	}
	.col-body {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		max-height: calc(100vh - 16rem);
		overflow-y: auto;
		padding: var(--sp-1);
	}
	.col-empty {
		display: flex;
		align-items: center;
		justify-content: center;
		padding: var(--sp-4);
		border: 1px dashed var(--border);
		border-radius: var(--r-md);
	}
</style>
