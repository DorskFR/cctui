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

{#each segments as seg (seg.key)}
	{#if seg.kind === 'rows'}
		<SessionRows {sp} rows={seg.rows} {childGroups} allowSelect={false} {hl} {pending} />
	{:else}
		{@const open = parentGroupOpen(sp.list.expanded, seg.parent.id, sp.searching)}
		<ParentGroupHeader
			parent={seg.parent}
			count={seg.rows.length}
			{open}
			ontoggle={() => sp.list.toggleGroup(seg.parent.id, parentGroupKey(sp.searching))}
			onopenparent={sp.openById}
		/>
		{#if open}
			<div class="parent-group-rows">
				<SessionRows {sp} rows={seg.rows} {childGroups} allowSelect={false} {hl} {pending} />
			</div>
		{/if}
	{/if}
{/each}

<style>
	.parent-group-rows {
		margin-left: var(--sp-2);
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
</style>
