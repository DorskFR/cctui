<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import {
		groupUnderParents,
		parentGroupKey,
		parentGroupOpen,
		type SubGroup
	} from './sessions.logic';
	import type { SessionsPage } from './sessionsPage.svelte';
	import ParentGroupHeader from './ParentGroupHeader.svelte';
	import SessionRows from './SessionRows.svelte';

	let {
		sp,
		rows,
		childGroups,
		hl,
		pending
	}: {
		sp: SessionsPage;
		rows: SessionListItem[];
		childGroups: Map<string, SubGroup[]>;
		hl: string[];
		pending: (id: string) => number;
	} = $props();

	const segments = $derived(groupUnderParents(rows, sp.pageParents));
</script>

{#snippet segmentRows()}
	{#each segments as seg (seg.key)}
		{#if seg.kind === 'rows'}
			<SessionRows {sp} rows={seg.rows} {childGroups} allowSelect={false} {hl} {pending} framed={false} />
		{:else}
			{@const open = parentGroupOpen(sp.list.expanded, seg.parent.id, sp.searching)}
			<ParentGroupHeader
				parent={seg.parent}
				session={sp.loadedById(seg.parent.id)}
				count={seg.rows.length}
				{open}
				ontoggle={() => sp.list.toggleGroup(seg.parent.id, parentGroupKey(sp.searching))}
				onopenparent={sp.openById}
			/>
			{#if open}
				<SessionRows
					{sp}
					rows={seg.rows}
					{childGroups}
					allowSelect={false}
					{hl}
					{pending}
					framed={false}
					baseDepth={1}
				/>
			{/if}
		{/if}
	{/each}
{/snippet}

{#if sp.cardView}
	{#each segments as seg (seg.key)}
		<SessionRows {sp} rows={seg.rows} {childGroups} allowSelect={false} {hl} {pending} />
	{/each}
{:else if segments.length > 0}
	<div class="frame"><div class="frame-in">{@render segmentRows()}</div></div>
{/if}

<style>
	.frame {
		border-radius: var(--r-md);
		overflow: hidden;
		background: var(--bg-elevated);
	}
	.frame-in {
		margin-bottom: -1px;
	}
</style>
