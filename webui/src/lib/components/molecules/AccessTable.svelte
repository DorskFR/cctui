<script lang="ts" module>
	export interface AccessColumn {
		key: string;
		label?: string;
		width: string;
	}
</script>

<script lang="ts" generics="T">
	import type { Snippet } from 'svelte';
	import { Spinner, Text } from '@dorsk/tsumikit';

	let {
		columns,
		rows,
		rowKey,
		row,
		bar,
		empty,
		loading = false,
		dim
	}: {
		columns: AccessColumn[];
		rows: T[];
		rowKey: (r: T) => string;
		row: Snippet<[T]>;
		bar?: Snippet;
		empty: string;
		loading?: boolean;
		dim?: (r: T) => boolean;
	} = $props();

	const template = $derived(columns.map((c) => c.width).join(' '));
</script>

<section class="tbl" style:--cols={template}>
	{#if bar}
		<div class="bar">{@render bar()}</div>
	{/if}
	<div class="hrow" aria-hidden="true">
		{#each columns as c (c.key)}<span>{c.label ?? ''}</span>{/each}
	</div>
	{#if loading}
		<div class="msg"><Spinner /></div>
	{:else if rows.length === 0}
		<div class="msg"><Text size="sm" tone="faint">{empty}</Text></div>
	{:else}
		<ul class="tbody">
			{#each rows as r (rowKey(r))}
				<li class="trow" class:dim={dim?.(r)}>{@render row(r)}</li>
			{/each}
		</ul>
	{/if}
</section>

<style>
	.tbl {
		--pad: 10px;
		--rowh: 48px;
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		overflow: hidden;
		overflow-x: auto;
	}
	.bar {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding: var(--pad) var(--sp-4);
		border-bottom: 1px solid var(--border);
	}
	.hrow,
	.trow {
		display: grid;
		grid-template-columns: var(--cols);
		gap: 0 var(--sp-3);
		align-items: center;
		padding-inline: var(--sp-4);
		min-width: 38rem;
	}
	.hrow {
		padding-block: 6px;
		font-size: 10.5px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-faint);
		border-bottom: 1px solid var(--border);
	}
	.tbody {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.trow {
		padding-block: var(--pad);
		min-height: var(--rowh);
		font-size: var(--fs-sm);
		border-bottom: 1px solid var(--border);
	}
	.trow:last-child {
		border-bottom: 0;
	}
	.trow:hover,
	.trow:focus-within {
		background: var(--bg-elevated-2);
	}
	.trow.dim {
		opacity: 0.55;
	}
	.msg {
		display: grid;
		place-items: center;
		padding: var(--sp-5);
	}
</style>
