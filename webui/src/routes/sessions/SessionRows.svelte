<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import SessionCard from '$lib/components/organisms/SessionCard.svelte';
	import { m } from '$lib/paraglide/messages';
	import { INLINE_THRESHOLD, costRollup, groupId, type SubGroup } from './sessions.logic';
	import type { SessionsPage } from './sessionsPage.svelte';

	// Every row set — live buckets, search results, archive browse — renders
	// through this one card-vs-list dispatch so no branch can drift from the
	// view picker. `allowSelect` gates the multi-select checkboxes (live buckets
	// only); `hl` carries search terms.
	let {
		sp,
		rows,
		childGroups,
		allowSelect,
		hl,
		pending
	}: {
		sp: SessionsPage;
		rows: SessionListItem[];
		childGroups: Map<string, SubGroup[]>;
		allowSelect: boolean;
		hl: string[];
		pending: (id: string) => number;
	} = $props();
</script>

{#if sp.cardView}
	<div class="card-grid">{@render cardItems(rows, hl)}</div>
{:else}
	{@render nestedRows(rows, hl)}
{/if}

<!-- Card (grid) view: top-level sessions laid out as detailed cards in a
     responsive grid. Subagents are omitted here (the list view + the drawer
     still show them); the point is at-a-glance status. -->
{#snippet cardItems(rows: SessionListItem[], hl: string[] = [], depth = 0)}
	{#each rows as s (s.id)}
		{@const subGroups = childGroups.get(s.id) ?? []}
		<SessionCard
			session={s}
			child={depth > 0}
			showMachine={sp.showMachine}
			accentHue={sp.accentOf(s)}
			stacked={subGroups.length > 0}
			pendingCount={pending(s.id)}
			unreadCount={sp.openSession?.id === s.id ? 0 : (s.unread_count ?? 0)}
			onopen={sp.openFromCard}
			selectable={sp.list.selecting}
			selected={sp.list.selected.has(s.id)}
			onToggleSelect={sp.list.toggleSelect}
			swipeable
			swipeLabel={m.sessions_archive()}
			onSwipe={sp.swipeArchive}
			onTogglePin={depth > 0 ? undefined : sp.togglePin}
			highlight={hl}
			subagentCost={costRollup(s, subGroups)}
			subagentToggles={subGroups.map((g) => ({
				key: g.key,
				count: g.agents.length,
				running: g.running,
				open: sp.list.expanded.has(groupId(s.id, g.key)),
				label: g.label,
				ontoggle: () => sp.list.toggleGroup(s.id, g.key)
			}))}
			allLabels={sp.allLabels}
			onCreateLabel={sp.createLabel}
			onAttachLabel={depth > 0 ? undefined : sp.attachLabel}
			onDetachLabel={depth > 0 ? undefined : sp.detachLabel}
			onUpdateLabel={sp.updateLabel}
			onDeleteLabel={sp.deleteLabel}
		/>
		<!-- Clicking a count badge expands that subagent group as cards inserted
		     right after the parent card in the grid flow. -->
		{#if depth < 5}
			{#each subGroups as g (g.key)}
				{#if sp.list.expanded.has(groupId(s.id, g.key))}
					{@render cardItems(g.agents, hl, depth + 1)}
				{/if}
			{/each}
		{/if}
	{/each}
{/snippet}

<!-- Nested list of top-level rows with subagent count badges + inline children. -->
{#snippet nestedRows(rows: SessionListItem[], hl: string[], depth = 0)}
	{#each rows as s (s.id)}
		{@const subGroups = childGroups.get(s.id) ?? []}
		{@const collapsibleGroups = subGroups.filter((g) => g.agents.length >= INLINE_THRESHOLD)}
		<!-- Collapsible (>=3) groups surface as count badges outside the parent
		     row layout; smaller groups render inline below. -->
		<div class="parent-row">
			<SessionCard
				session={s}
				variant="row"
				child={depth > 0}
				showMachine={sp.showMachine}
				accentHue={sp.accentOf(s)}
				pendingCount={pending(s.id)}
				unreadCount={sp.openSession?.id === s.id ? 0 : (s.unread_count ?? 0)}
				onopen={sp.openFromCard}
				selectable={allowSelect && sp.list.selecting}
				selected={sp.list.selected.has(s.id)}
				onToggleSelect={sp.list.toggleSelect}
				swipeable
				swipeLabel={s.status === 'archived' ? m.sessions_unarchive() : m.sessions_archive()}
				onSwipe={sp.swipeArchive}
				onTogglePin={depth > 0 ? undefined : sp.togglePin}
				highlight={hl}
				subagentCost={costRollup(s, subGroups)}
				subagentToggles={collapsibleGroups.map((g) => ({
					key: g.key,
					count: g.agents.length,
					running: g.running,
					open: sp.list.expanded.has(groupId(s.id, g.key)),
					label: g.label,
					ontoggle: () => sp.list.toggleGroup(s.id, g.key)
				}))}
				allLabels={sp.allLabels}
				onCreateLabel={sp.createLabel}
				onAttachLabel={depth > 0 ? undefined : sp.attachLabel}
				onDetachLabel={depth > 0 ? undefined : sp.detachLabel}
				onUpdateLabel={sp.updateLabel}
				onDeleteLabel={sp.deleteLabel}
			/>
		</div>
		{#if depth < 5}
			{#each subGroups as g (g.key)}
				{#if g.agents.length < INLINE_THRESHOLD || sp.list.expanded.has(groupId(s.id, g.key))}
					<div class="agent-children" style="--agent-depth: {Math.min(depth + 1, 5)}">
						{@render nestedRows(g.agents, hl, depth + 1)}
					</div>
				{/if}
			{/each}
		{/if}
	{/each}
{/snippet}

<style>
	/* Parent row: a normal full-width row. The collapse toggle badge(s) live
	   inside the card's leading gutter slot (SessionCard), so there's no
	   external rail and no reserved left gutter to keep aligned across sections. */
	.parent-row {
		position: relative;
	}
	.agent-children {
		margin-left: min(calc(var(--agent-depth) * var(--sp-2)), 2.5rem);
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	@media (max-width: 639px) {
		.agent-children {
			margin-left: min(calc(var(--agent-depth) * var(--sp-1)), 1.25rem);
		}
	}
	/* Detailed cards auto-fill the strip: never narrower than a compact card,
	   capped so a wide window packs more columns instead of stretching them,
	   and each row takes its tallest card's natural height. */
	.card-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(min(100%, 20rem), 26.75rem));
		justify-content: start;
		align-items: stretch;
		gap: var(--sp-3);
	}
	/* One column takes the whole strip: a capped track leaves a gutter on phones. */
	@container (max-width: 40rem) {
		.card-grid {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
