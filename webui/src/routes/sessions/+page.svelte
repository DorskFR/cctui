<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { useSessions, useSessionActions, endpoints, SYSTEM_MACHINE_KINDS } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { toasts } from '$lib/toast.svelte';
	import { ws } from '$lib/ws.svelte';
	import SessionCard from '$lib/components/SessionCard.svelte';
	import ConversationDrawer from '$lib/components/ConversationDrawer.svelte';
	import SpawnModal from '$lib/components/SpawnModal.svelte';
	import { drafts, LIST_DENSITY } from '$lib/drafts';
	import { notify } from '$lib/notify.svelte';
	import { tokenizeQuery } from '$lib/search';

	let dense = $state(drafts.get(LIST_DENSITY) === 'compact');
	$effect(() => {
		drafts.set(LIST_DENSITY, dense ? 'compact' : 'normal');
	});

	let showArchived = $state(false);
	let openSession = $state<SessionListItem | null>(null);
	let showSpawn = $state(false);

	// Live buckets always show non-archived sessions; the archive is a separate
	// paginated section below (CCT-184).
	const sessions = useSessions(() => false);

	const qc = useQueryClient();
	const actions = useSessionActions();

	// ── Search + archive browse (CCT-184) ──────────────────────────────────
	// One paginated "pager" feeds two views, never both at once:
	//   • searching (q non-empty) → search results, scoped by `showArchived`
	//     (unticked = live only, ticked = all). Split into Live / Archived.
	//   • not searching + showArchived → browse the archive (empty q), paged.
	// Live-only with no query needs no pager — the bucketed list owns it.
	const PAGE = 50;
	let rawQuery = $state('');
	let query = $state('');
	$effect(() => {
		const v = rawQuery.trim();
		const t = setTimeout(() => (query = v), 200);
		return () => clearTimeout(t);
	});
	const searching = $derived(query.length > 0);
	const pagerActive = $derived(searching || showArchived);
	// Parsed terms to highlight in result snippets + the opened chat (CCT-187).
	const searchTerms = $derived(tokenizeQuery(query));

	let pageRows = $state<SessionListItem[]>([]);
	let pageOffset = $state(0);
	let pageDone = $state(false);
	let pageLoading = $state(false);
	let pageError = $state('');
	let pageReqId = 0; // discard out-of-order/superseded responses
	let refreshTick = $state(0); // bump to reload the pager after archive ops

	async function loadPage(reset: boolean) {
		if (!pagerActive) return;
		const offset = reset ? 0 : pageOffset;
		const req = ++pageReqId;
		pageLoading = true;
		pageError = '';
		try {
			// not searching ⇒ browse archive (empty q, archived scope).
			const res = await endpoints.searchSessions(
				query,
				searching ? showArchived : true,
				PAGE,
				offset
			);
			if (req !== pageReqId) return;
			const rows = res.sessions;
			pageRows = reset ? rows : [...pageRows, ...rows];
			pageOffset = offset + rows.length;
			pageDone = rows.length < PAGE;
		} catch (e) {
			if (req === pageReqId) pageError = (e as Error).message;
		} finally {
			if (req === pageReqId) pageLoading = false;
		}
	}

	// Reset + reload page 0 whenever the mode (query / scope) changes, or an
	// archive action bumps refreshTick.
	const pagerKey = $derived(`${searching ? `q:${query}` : 'browse'}|${showArchived}`);
	$effect(() => {
		void pagerKey;
		void refreshTick;
		pageRows = [];
		pageOffset = 0;
		pageDone = false;
		pageError = '';
		if (pagerActive) loadPage(true);
	});

	const liveResults = $derived(pageRows.filter((s) => s.status !== 'archived'));
	const archivedResults = $derived(pageRows.filter((s) => s.status === 'archived'));

	// ── Multi-select / batch archive (CCT-172) ─────────────────────────────
	// Applies to the live buckets only (always non-archived → always archives).
	let selecting = $state(false);
	let selected = $state(new Set<string>());
	let archiving = $state(false);

	function toggleSelect(s: SessionListItem) {
		const next = new Set(selected);
		if (next.has(s.id)) next.delete(s.id);
		else next.add(s.id);
		selected = next;
	}
	function exitSelect() {
		selecting = false;
		selected = new Set();
	}
	function selectAll() {
		selected = new Set(items.map((s) => s.id));
	}
	async function archiveSelected() {
		const ids = [...selected];
		if (ids.length === 0) return;
		if (ids.length > 1 && !confirm(`Archive ${ids.length} sessions?`)) return;
		archiving = true;
		try {
			await actions.archiveMany(ids);
			toasts.ok(`Archived ${ids.length}`);
			exitSelect();
			refreshTick++;
		} catch (e) {
			toasts.err((e as Error).message);
		} finally {
			archiving = false;
		}
	}

	// Swipe-to-archive a single row (CCT-172). Status-aware so it works for both
	// live (archive) and archived (unarchive) rows.
	async function swipeArchive(s: SessionListItem) {
		const isArchived = s.status === 'archived';
		try {
			if (isArchived) await actions.unarchive(s.id);
			else await actions.archive(s.id);
			toasts.ok(isArchived ? 'Unarchived' : 'Archived');
			refreshTick++;
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	// live status changes from the websocket → refetch the list
	$effect(() => {
		void ws.changeTick;
		qc.invalidateQueries({ queryKey: ['sessions'] });
	});

	// Tell the notifier which drawer is open so it won't notify for it.
	$effect(() => {
		notify.openSessionId = openSession?.id ?? null;
		return () => {
			notify.openSessionId = null;
		};
	});

	// A clicked notification asks us to open its session's drawer.
	$effect(() => {
		const id = notify.pendingOpen;
		if (!id) return;
		const target = [...items, ...pageRows].find((s) => s.id === id);
		if (target) {
			openSession = target;
			notify.pendingOpen = null;
		}
	});

	const items = $derived($sessions.data?.sessions ?? []);
	const ids = $derived(new Set(items.map((s) => s.id)));
	const childrenOf = $derived.by(() => {
		const map = new Map<string, SessionListItem[]>();
		for (const s of items) {
			if (s.parent_id && ids.has(s.parent_id)) {
				map.set(s.parent_id, [...(map.get(s.parent_id) ?? []), s]);
			}
		}
		return map;
	});
	const topLevel = $derived(items.filter((s) => !s.parent_id || !ids.has(s.parent_id)));

	// Classifier buckets (CCT-90), in attention-first display order. Sessions
	// that want the user's eyes float to the top; empty buckets are dropped.
	// Sessions on server-managed machines (dispatch / ephemeral workers) get
	// their own "Dispatched" group at the bottom (CCT-231) — they're unattended
	// noise next to interactive sessions — EXCEPT blocked ones, which still
	// surface under Needs input so attention never gets buried.
	type GroupKey = SessionListItem['bucket'] | 'dispatched';
	const BUCKETS: { key: GroupKey; label: string }[] = [
		{ key: 'blocked', label: 'Needs input' },
		{ key: 'review', label: 'Ready for review' },
		{ key: 'working', label: 'Working' },
		{ key: 'done', label: 'Completed' },
		{ key: 'dispatched', label: 'Dispatched' }
	];
	const isDispatched = (s: SessionListItem) =>
		s.machine_kind != null && SYSTEM_MACHINE_KINDS.has(s.machine_kind);
	const groupOf = (s: SessionListItem): GroupKey => {
		const bucket = s.bucket ?? 'working';
		if (bucket === 'blocked') return 'blocked';
		return isDispatched(s) ? 'dispatched' : bucket;
	};
	const groups = $derived(
		BUCKETS.map((b) => ({
			...b,
			sessions: topLevel.filter((s) => groupOf(s) === b.key)
		})).filter((g) => g.sessions.length > 0)
	);

	const pending = (id: string) => {
		void ws.changeTick; // re-derive when perms change (setPerms bumps changeTick)
		return ws.pendingCount(id);
	};

	// keep the open drawer's session object fresh as the list refetches
	const liveOpen = $derived(
		openSession
			? ([...items, ...pageRows].find((s) => s.id === openSession!.id) ?? openSession)
			: null
	);
</script>

<div class="bar row">
	<h1 class="page-title">Sessions</h1>
	<input class="search" type="search" placeholder="Search all chats…" bind:value={rawQuery} />
	<label class="arch row">
		<input type="checkbox" bind:checked={showArchived} /> Archived
	</label>
	<button class="btn btn-sm" title="Toggle compact / detailed rows" onclick={() => (dense = !dense)}
		>{dense ? '☰ Compact' : '▤ Detailed'}</button
	>
	{#if !searching}
		{#if selecting}
			<button class="btn btn-sm" onclick={exitSelect}>Cancel</button>
		{:else}
			<button class="btn btn-sm" title="Select multiple to archive" onclick={() => (selecting = true)}
				>☑ Select</button
			>
		{/if}
	{/if}
	<button class="btn btn-primary btn-sm" onclick={() => (showSpawn = true)}>+ New</button>
</div>

{#if selecting && !searching}
	<div class="bulkbar row">
		<span class="count">{selected.size} selected</span>
		<button class="btn btn-sm" onclick={selectAll}>Select all</button>
		<div class="spacer"></div>
		<button
			class="btn btn-sm btn-danger"
			disabled={selected.size === 0 || archiving}
			onclick={archiveSelected}
		>
			{#if archiving}<span class="spin"></span>{/if}
			Archive {selected.size || ''}
		</button>
	</div>
{/if}

{#snippet pager(rows: SessionListItem[])}
	{#each rows as s (s.id)}
		<SessionCard
			session={s}
			compact={dense}
			pendingCount={pending(s.id)}
			onopen={(x) => (openSession = x)}
			swipeable
			swipeLabel={s.status === 'archived' ? 'Unarchive' : 'Archive'}
			onSwipe={swipeArchive}
			highlight={searchTerms}
		/>
	{/each}
{/snippet}

{#snippet loadMore()}
	{#if pageError}
		<div class="empty err">Search failed: {pageError}</div>
	{:else if pageLoading}
		<div class="loadmore"><span class="spin"></span></div>
	{:else if !pageDone && pageRows.length > 0}
		<div class="loadmore">
			<button class="btn btn-sm" onclick={() => loadPage(false)}>Load more</button>
		</div>
	{/if}
{/snippet}

{#if searching}
	<!-- Search results, scoped by the Archived checkbox; split Live / Archived. -->
	{#if pageLoading && pageRows.length === 0}
		<div class="empty"><span class="spin"></span></div>
	{:else if pageRows.length === 0}
		<div class="empty">No chats match “{query}”{showArchived ? '' : ' (live only — tick Archived to search all)'}.</div>
	{:else}
		<div class="stack" class:tight={dense}>
			{#if liveResults.length > 0}
				<div class="group-header">Live <span class="count">{liveResults.length}</span></div>
				{@render pager(liveResults)}
			{/if}
			{#if archivedResults.length > 0}
				<div class="group-header">Archived <span class="count">{archivedResults.length}</span></div>
				{@render pager(archivedResults)}
			{/if}
			{@render loadMore()}
		</div>
	{/if}
{:else}
	<!-- Live buckets first… -->
	{#if $sessions.isLoading}
		<div class="empty"><span class="spin"></span></div>
	{:else if topLevel.length === 0 && !showArchived}
		<div class="empty">No sessions — tick Archived or start one.</div>
	{:else}
		<div class="stack" class:tight={dense}>
			{#each groups as g (g.key)}
				<div class="group-header" data-bucket={g.key}>
					{g.label} <span class="count">{g.sessions.length}</span>
				</div>
				{#each g.sessions as s (s.id)}
					<SessionCard
						session={s}
						compact={dense}
						pendingCount={pending(s.id)}
						onopen={(x) => (openSession = x)}
						selectable={selecting}
						selected={selected.has(s.id)}
						onToggleSelect={toggleSelect}
						swipeable
						swipeLabel="Archive"
						onSwipe={swipeArchive}
					/>
					{#each childrenOf.get(s.id) ?? [] as c (c.id)}
						<SessionCard
							session={c}
							child
							compact={dense}
							pendingCount={pending(c.id)}
							onopen={(x) => (openSession = x)}
							selectable={selecting}
							selected={selected.has(c.id)}
							onToggleSelect={toggleSelect}
							swipeable
							swipeLabel="Archive"
							onSwipe={swipeArchive}
						/>
					{/each}
				{/each}
			{/each}
		</div>
	{/if}

	<!-- …then the paginated archive when requested. -->
	{#if showArchived}
		<div class="stack" class:tight={dense}>
			<div class="group-header">Archived <span class="count">{pageRows.length}</span></div>
			{#if pageRows.length === 0 && !pageLoading}
				<div class="empty">No archived sessions.</div>
			{:else}
				{@render pager(pageRows)}
				{@render loadMore()}
			{/if}
		</div>
	{/if}
{/if}

{#if liveOpen}
	<ConversationDrawer
		session={liveOpen}
		onclose={() => (openSession = null)}
		highlight={searchTerms}
	/>
{/if}

{#if showSpawn}
	<SpawnModal
		onclose={() => (showSpawn = false)}
		onspawned={() => qc.invalidateQueries({ queryKey: ['sessions'] })}
	/>
{/if}

<style>
	.bar {
		margin-bottom: var(--sp-4);
		gap: var(--sp-2);
		align-items: stretch;
	}
	.page-title {
		font-size: var(--fs-2xl);
		align-self: center;
	}
	.arch {
		font-size: var(--fs-sm);
		color: var(--text-muted);
		gap: var(--sp-1);
		align-self: center;
		white-space: nowrap;
	}
	/* Fill all the space between the title and the Archived checkbox. */
	.search {
		flex: 1;
		min-width: 0;
		padding: var(--sp-1) var(--sp-3);
		font-size: var(--fs-sm);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		color: var(--text);
	}
	/* Sticky bulk-action bar (CCT-172) shown while in select mode. */
	.bulkbar {
		position: sticky;
		top: 0;
		z-index: 5;
		gap: var(--sp-2);
		align-items: center;
		margin-bottom: var(--sp-3);
		padding: var(--sp-2) var(--sp-3);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		box-shadow: var(--shadow-md);
	}
	.bulkbar .count {
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
		color: var(--text-muted);
	}
	.stack.tight {
		gap: var(--sp-1);
	}
	.loadmore {
		display: flex;
		justify-content: center;
		padding: var(--sp-3) 0;
	}
	.empty.err {
		color: var(--danger, #bf616a);
	}
	.group-header {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		margin-top: var(--sp-3);
		font-size: var(--fs-sm);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--text-muted);
	}
	.group-header:first-child {
		margin-top: 0;
	}
	.group-header .count {
		font-weight: 400;
		opacity: 0.7;
	}
	.group-header[data-bucket='blocked'] {
		color: var(--warn, #d08770);
	}
	.group-header[data-bucket='review'] {
		color: var(--accent, #88c0d0);
	}
</style>
