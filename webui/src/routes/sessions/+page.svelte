<script lang="ts">
	import { untrack } from 'svelte';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { useSessions, useSessionActions, endpoints, SYSTEM_MACHINE_KINDS } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { page } from '$app/state';
	import { pushState, replaceState } from '$app/navigation';
	import { toasts } from '$lib/toast.svelte';
	import { ws } from '$lib/ws.svelte';
	import SessionCard from '$lib/components/SessionCard.svelte';
	import WorkflowGroup from '$lib/components/WorkflowGroup.svelte';
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

	// ── Deep-linkable session (CCT-206) ─────────────────────────────────────
	// A session's stable, shareable URL is /sessions?session=<id>. The whole SPA
	// already sits behind the login wall (layout renders <Login/> when unauthed
	// and keeps the URL intact), so following a shared link while logged out shows
	// the login wall and lands on the requested session right after auth — the
	// "return to intended destination" is free as long as we never redirect away.
	//
	// `openSession` is the source of truth for the drawer; the URL mirrors it.
	// `openById` opens a session by id, pulling it from the loaded lists or, if
	// absent (purged from the live view, on another page, etc.), fetching it
	// directly so a pasted link still resolves. We track the last id we synced to
	// the URL so list refetches (which churn the session object) don't re-push.
	let lastUrlId: string | null = null;
	let urlResolving = false;

	function setUrlSession(id: string | null, replace = false) {
		const url = new URL(page.url);
		if (id) url.searchParams.set('session', id);
		else url.searchParams.delete('session');
		if (url.href === page.url.href) return;
		if (replace) replaceState(url, {});
		else pushState(url, {});
	}

	async function openById(id: string) {
		const found = [...items, ...pageRows].find((s) => s.id === id);
		if (found) {
			openSession = found;
			return;
		}
		urlResolving = true;
		try {
			openSession = await endpoints.session(id);
		} catch {
			toasts.err('Session not found or archived.');
			openSession = null;
			// drop the dangling id from the URL so a reload doesn't re-toast
			setUrlSession(null, true);
		} finally {
			urlResolving = false;
		}
	}

	// URL → drawer: react to the `session` param (initial load, back/forward,
	// pasted link). Only act when it differs from what's already open. The
	// `openSession` read is untracked (CCT-240): if this effect depended on it,
	// any `openSession = …` (card click, notification) would re-run it *before*
	// the drawer→URL effect below pushes `?session=<id>` — the still-empty URL
	// param then hit the `openSession = null` branch and closed the drawer in
	// the same flush, so conversations never opened. Depending only on the URL
	// keeps this effect to its job: URL changes drive the drawer, not vice versa.
	$effect(() => {
		const id = page.url.searchParams.get('session');
		if (id === untrack(() => openSession?.id ?? null)) return;
		if (id) void openById(id);
		else openSession = null;
	});

	// drawer → URL: reflect the open session into the address bar so it's always
	// a shareable link. Skip while we're resolving a URL-driven open (no echo).
	$effect(() => {
		const id = openSession?.id ?? null;
		if (urlResolving) return;
		if (id === lastUrlId) return;
		// first reflection on load uses replace (don't trap back); thereafter push
		setUrlSession(id, lastUrlId === null);
		lastUrlId = id;
	});

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
	// Mobile (narrow viewports): the search input collapses to a magnifier
	// button and expands over the toolbar on tap (CCT-241). Desktop ignores
	// `searchOpen` entirely — the input always fills the bar there.
	let searchOpen = $state(false);
	let searchEl = $state<HTMLInputElement | null>(null);
	function openSearch() {
		searchOpen = true;
		// Focus after the expand transition kicks in so the keyboard pops.
		requestAnimationFrame(() => searchEl?.focus());
	}
	function onSearchBlur() {
		if (!rawQuery.trim()) searchOpen = false;
	}
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
		} else {
			// not in any loaded list — fetch it by id so the notification still opens
			void openById(id);
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

	// Workflow-tool subagents (CCT-225) carry a `workflow_run_id` in their
	// session metadata. A single run can spawn 100+ agents, so rather than
	// dumping them all as flat children we group them by run id under a
	// collapsible "Workflow: <name> (<runId>)" header. Plain (Task-tool)
	// children render as before. Returns, per parent id, the ungrouped children
	// plus an ordered list of workflow groups.
	type WfGroup = { runId: string; name: string | null; agents: SessionListItem[] };
	function metaStr(s: SessionListItem, key: string): string | null {
		const m = s.metadata as Record<string, unknown> | null;
		const v = m?.[key];
		return typeof v === 'string' ? v : null;
	}
	const childGroupsOf = $derived.by(() => {
		const map = new Map<string, { plain: SessionListItem[]; workflows: WfGroup[] }>();
		for (const [parentId, kids] of childrenOf) {
			const plain: SessionListItem[] = [];
			const byRun = new Map<string, WfGroup>();
			for (const k of kids) {
				const runId = metaStr(k, 'workflow_run_id');
				if (runId) {
					let g = byRun.get(runId);
					if (!g) {
						g = { runId, name: metaStr(k, 'workflow_name'), agents: [] };
						byRun.set(runId, g);
					}
					g.agents.push(k);
				} else {
					plain.push(k);
				}
			}
			map.set(parentId, { plain, workflows: [...byRun.values()] });
		}
		return map;
	});

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
	<button class="btn btn-sm search-toggle" aria-label="Search chats" onclick={openSearch}>
		<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
			<circle cx="11" cy="11" r="7" />
			<path d="m21 21-4.3-4.3" />
		</svg>
	</button>
	<input
		class="search"
		class:open={searchOpen}
		type="search"
		placeholder="Search all chats…"
		bind:value={rawQuery}
		bind:this={searchEl}
		onblur={onSearchBlur}
		onkeydown={(e) => {
			if (e.key === 'Escape') {
				rawQuery = '';
				searchOpen = false;
				(e.currentTarget as HTMLInputElement).blur();
			}
		}}
	/>
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
					{#each childGroupsOf.get(s.id)?.plain ?? [] as c (c.id)}
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
					{#each childGroupsOf.get(s.id)?.workflows ?? [] as g (g.runId)}
						<WorkflowGroup
							runId={g.runId}
							name={g.name}
							agents={g.agents}
							compact={dense}
							pending={pending}
							onopen={(x) => (openSession = x)}
							selecting={selecting}
							selected={selected}
							onToggleSelect={toggleSelect}
							swipeArchive={swipeArchive}
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
		/* Sticky under the fixed app header so search/density/New stay reachable
		   on long lists without scrolling back up (CCT-241). Also the positioning
		   context for the mobile search overlay. */
		position: sticky;
		top: calc(var(--header-h) + var(--safe-top));
		z-index: 6;
		margin-bottom: var(--sp-4);
		padding: var(--sp-2) 0;
		gap: var(--sp-2);
		align-items: stretch;
		background: var(--bg);
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
	/* Desktop: the input always fills the bar; no toggle button. */
	.search-toggle {
		display: none;
		align-self: center;
	}
	/* Mobile: collapsed to a magnifier; tapping it expands the input over the
	   whole toolbar with a smooth width/opacity transition (CCT-241). */
	@media (max-width: 639px) {
		.search-toggle {
			display: inline-flex;
			align-items: center;
		}
		.search {
			position: absolute;
			inset: var(--sp-2) 0;
			width: 2.25rem;
			margin-left: auto;
			opacity: 0;
			pointer-events: none;
			transition:
				width 0.2s var(--ease),
				opacity 0.15s var(--ease);
		}
		.search.open {
			width: 100%;
			opacity: 1;
			pointer-events: auto;
		}
	}
	/* Sticky bulk-action bar (CCT-172) shown while in select mode. */
	.bulkbar {
		position: sticky;
		top: calc(var(--header-h) + var(--safe-top) + var(--sp-2));
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
