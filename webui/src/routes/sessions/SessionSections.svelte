<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Button, Callout, Container, Spinner, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { nest, idsForSection } from './sessions.logic';
	import type { SessionsPage } from './sessionsPage.svelte';
	import SessionGroupHeader from './SessionGroupHeader.svelte';
	import SessionRows from './SessionRows.svelte';
	import DraftRows from './DraftRows.svelte';

	// The session list body: live buckets (or group-by sections), drafts, and
	// the paged archive browse / search results, each under a group header.
	let {
		sp,
		pending,
		machineLiveness
	}: {
		sp: SessionsPage;
		pending: (id: string) => number;
		machineLiveness: (name: string) => 'online' | 'stale' | 'offline' | null;
	} = $props();
</script>

<!-- Shared section wrapper: card-detailed fills the content column, which the
     layout has already narrowed by whatever the docked panels reserve; every
     other view stays centered. `fullWidth` would break out to the viewport and
     reserve the docks a second time. -->
{#snippet sectionsWrap(body: Snippet)}
	{#if sp.cardView}
		<Container size="none">
			<div class="sections" data-journey="session-list">{@render body()}</div>
		</Container>
	{:else}
		<div class="sections tight" data-journey="session-list">{@render body()}</div>
	{/if}
{/snippet}

{#snippet loadMore()}
	{#if sp.pageError}
		<Callout tone="danger">{m.sessions_search_failed({ error: sp.pageError })}</Callout>
	{:else if sp.pageLoading}
		<div class="loadmore"><Spinner label={m.common_loading()} /></div>
		{:else if !sp.pageDone && sp.pageRows.length > 0}
			<div class="loadmore">
				<Button onclick={() => sp.loadPage(false)}>{m.sessions_load_more()}</Button>
			</div>
		{/if}
{/snippet}

<!-- The height floor keeps the document from collapsing under the sticky
     toolbar while a search swaps the tall live list for a short results block —
     without it the window scrollTop clamps and the bar jumps mid-type. -->
<div class="list-area">
	{#if sp.searching}
		<!-- Search results, scoped by the Archived checkbox; split Live / Archived. -->
		{#if sp.pageLoading && sp.pageRows.length === 0}
			<div class="placeholder"><Spinner label={m.common_loading()} /></div>
		{:else if sp.pageRows.length === 0}
			<div class="placeholder"><Text tone="muted">{m.sessions_search_no_match({ query: sp.serverQuery })}{sp.showArchived ? '.' : ' ' + m.sessions_search_live_only_hint()}</Text></div>
		{:else}
			{@render sectionsWrap(searchSections)}
		{/if}
	{:else}
		{@render sectionsWrap(liveSections)}
	{/if}
</div>

{#snippet searchSections()}
	<!-- Nest over the whole result set so a parent and its subagents stay
	     grouped even if they land in different status sections; then split
	     the top-level rows into Live / Archived. -->
	{@const ns = nest(sp.pageRows)}
	{@const scoped = ns.topLevel.filter(sp.keepRow)}
	{@const liveTop = scoped.filter((s) => s.status !== 'archived')}
	{@const archTop = scoped.filter((s) => s.status === 'archived')}
	<div class="section">
		{@render groupHeader('live', m.sessions_section_live(), liveTop.length, {
			archiveIds: idsForSection(liveTop, ns.childGroups)
		})}
		{#if !sp.hiddenSections.has('live')}
			<SessionRows {sp} rows={liveTop} childGroups={ns.childGroups} allowSelect={false} hl={sp.searchTerms} {pending} />
		{/if}
	</div>
	{#if sp.showArchived}
		<div class="section">
			{@render groupHeader('archived', m.sessions_section_archived(), archTop.length, {})}
			{#if !sp.hiddenSections.has('archived')}
				<SessionRows {sp} rows={archTop} childGroups={ns.childGroups} allowSelect={false} hl={sp.searchTerms} {pending} />
			{/if}
		</div>
	{/if}
	{#if scoped.length === 0}
		<div class="placeholder"><Text tone="muted">{m.sessions_search_no_sections()}</Text></div>
	{/if}
	{@render loadMore()}
{/snippet}

{#snippet groupHeader(
	key: string,
	label: string,
	count: number,
	opts: { hue?: number | null; bucket?: string | null; machine?: string | null; archiveIds?: string[] }
)}
	<SessionGroupHeader
		{sp}
		{key}
		{label}
		{count}
		hue={opts.hue}
		bucket={opts.bucket}
		machine={opts.machine}
		liveness={opts.machine ? machineLiveness(opts.machine) : null}
		archiveIds={opts.archiveIds}
	/>
{/snippet}

{#snippet liveSections()}
		{#if sp.sessionsLoading}
			<div class="placeholder"><Spinner label={m.common_loading()} /></div>
		{:else if !sp.list.hasLiveRows && !sp.showArchived && !sp.sections.has('drafts')}
			<div class="placeholder">
				<Text tone="muted">{m.sessions_empty_sections()}</Text>
			</div>
		{:else if sp.groupBy !== 'status'}
			{#each sp.list.groupedSections as g (g.key)}
				{@const key = `dim:${g.key}`}
				<div class="section">
					{@render groupHeader(key, g.label, g.sessions.length, {
						hue: g.hue,
						machine: sp.groupBy === 'machine' && g.hue !== null ? g.label : null,
						archiveIds: idsForSection(g.sessions, sp.childGroupsOf)
					})}
					{#if !sp.hiddenSections.has(key)}
						<SessionRows {sp} rows={g.sessions} childGroups={sp.childGroupsOf} allowSelect={true} hl={[]} {pending} />
					{/if}
				</div>
			{/each}
		{:else}
			{#each sp.list.groups as g (g.key)}
				<div class="section" data-journey="section" data-journey-key={g.key}>
					{@render groupHeader(g.key, g.label, g.sessions.length, {
						bucket: g.key,
						archiveIds:
							g.key !== 'done' || sp.archiveDoneButton
								? idsForSection(g.sessions, sp.childGroupsOf)
								: undefined
					})}
					{#if !sp.hiddenSections.has(g.key)}
						<SessionRows {sp} rows={g.sessions} childGroups={sp.childGroupsOf} allowSelect={true} hl={[]} {pending} />
					{/if}
				</div>
			{/each}
		{/if}

		{#if sp.sections.has('drafts')}
			<div class="section" data-journey="section" data-journey-key="drafts">
				{@render groupHeader('drafts', m.sessions_section_drafts(), sp.list.draftRows.length, {})}
				{#if !sp.hiddenSections.has('drafts')}
					<DraftRows {sp} rows={sp.list.draftRows} />
				{/if}
			</div>
		{/if}

		{#if sp.showArchived}
			{@const ns = nest(sp.pageRows)}
			{@const archTop = ns.topLevel.filter(
				(s) => sp.keepRow(s) && !sp.pinnedArchivedKidIds.has(s.id)
			)}
			<div class="section">
				{@render groupHeader('archived', m.sessions_section_archived(), archTop.length, {})}
				{#if !sp.hiddenSections.has('archived')}
					{#if sp.pageRows.length === 0 && !sp.pageLoading}
						<div class="placeholder"><Text tone="muted">{m.sessions_no_archived()}</Text></div>
					{:else}
						<SessionRows {sp} rows={archTop} childGroups={ns.childGroups} allowSelect={false} hl={sp.searchTerms} {pending} />
						{@render loadMore()}
					{/if}
				{/if}
			</div>
		{/if}
{/snippet}

<style>
	/* Height floor so swapping the live list for short search results can't
	   shrink the document under the sticky bar and clamp the scroll position. */
	.list-area {
		min-height: 60vh;
	}
	/* Two-axis spacing: the outer container owns the inter-section gap;
	   each .section owns its row gap. Every section break — Pinned, Working,
	   Dispatched, Archived — is the same sp-6, with no header margins or
	   sibling-combinator patches that broke whenever Archived was its own block. */
	.sections {
		display: flex;
		flex-direction: column;
		gap: var(--sp-6);
	}
	.section {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.sections.tight .section {
		gap: var(--sp-1);
	}
	.loadmore {
		display: flex;
		justify-content: center;
		padding: var(--sp-3) 0;
	}
	.sections {
		container-type: inline-size;
	}
</style>
