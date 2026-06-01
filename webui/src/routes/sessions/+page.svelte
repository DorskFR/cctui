<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { useSessions, useSessionSearch, useSessionActions } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { toasts } from '$lib/toast.svelte';
	import { ws } from '$lib/ws.svelte';
	import SessionCard from '$lib/components/SessionCard.svelte';
	import ConversationDrawer from '$lib/components/ConversationDrawer.svelte';
	import SpawnModal from '$lib/components/SpawnModal.svelte';
	import { drafts, LIST_DENSITY } from '$lib/drafts';
	import { notify } from '$lib/notify.svelte';

	let dense = $state(drafts.get(LIST_DENSITY) === 'compact');
	$effect(() => {
		drafts.set(LIST_DENSITY, dense ? 'compact' : 'normal');
	});

	let showArchived = $state(false);
	let openSession = $state<SessionListItem | null>(null);
	let showSpawn = $state(false);

	const sessions = useSessions(() => showArchived);

	// ── Full-transcript search (CCT-184) ──────────────────────────────────
	// Debounce keystrokes into `query`; a non-empty query flips the page into
	// search mode (results split into Live / Archived), bypassing the bucketed
	// live list and its 25-row cap.
	let rawQuery = $state('');
	let query = $state('');
	$effect(() => {
		const v = rawQuery.trim();
		const t = setTimeout(() => (query = v), 200);
		return () => clearTimeout(t);
	});
	const searching = $derived(query.length > 0);
	const search = useSessionSearch(() => query);
	const searchResults = $derived($search.data?.sessions ?? []);
	const liveResults = $derived(searchResults.filter((s) => s.status !== 'archived'));
	const archivedResults = $derived(searchResults.filter((s) => s.status === 'archived'));

	const qc = useQueryClient();
	const actions = useSessionActions();

	// ── Multi-select / batch archive (CCT-172) ─────────────────────────────
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
		// Every currently-visible session (top-level + their shown subagents).
		selected = new Set(items.map((s) => s.id));
	}
	async function archiveSelected() {
		const ids = [...selected];
		if (ids.length === 0) return;
		if (ids.length > 1 && !confirm(`Archive ${ids.length} sessions?`)) return;
		archiving = true;
		try {
			if (showArchived) await actions.unarchiveMany(ids);
			else await actions.archiveMany(ids);
			toasts.ok(`${showArchived ? 'Unarchived' : 'Archived'} ${ids.length}`);
			exitSelect();
		} catch (e) {
			toasts.err((e as Error).message);
		} finally {
			archiving = false;
		}
	}

	// Swipe-to-archive a single row (CCT-172): the email-style left-swipe on a
	// SessionCard archives it (or unarchives in the archived view). Disabled
	// while in multi-select mode (handled in SessionCard).
	async function swipeArchive(s: SessionListItem) {
		// Decide from the row's own status, not the view: search results mix
		// live + archived sessions in one list.
		const isArchived = s.status === 'archived';
		try {
			if (isArchived) await actions.unarchive(s.id);
			else await actions.archive(s.id);
			toasts.ok(isArchived ? 'Unarchived' : 'Archived');
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
		const target = items.find((s) => s.id === id);
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
	const BUCKETS: { key: SessionListItem['bucket']; label: string }[] = [
		{ key: 'blocked', label: 'Needs input' },
		{ key: 'review', label: 'Ready for review' },
		{ key: 'working', label: 'Working' },
		{ key: 'done', label: 'Completed' }
	];
	const groups = $derived(
		BUCKETS.map((b) => ({
			...b,
			sessions: topLevel.filter((s) => (s.bucket ?? 'working') === b.key)
		})).filter((g) => g.sessions.length > 0)
	);

	const pending = (id: string) => {
		void ws.changeTick; // re-derive when perms change (setPerms bumps changeTick)
		return ws.pendingCount(id);
	};

	// keep the open drawer's session object fresh as the list refetches
	const liveOpen = $derived(
		openSession ? (items.find((s) => s.id === openSession!.id) ?? openSession) : null
	);
</script>

<div class="bar row">
	<h1 class="page-title">Sessions</h1>
	<input
		class="search"
		type="search"
		placeholder="Search all chats…"
		bind:value={rawQuery}
	/>
	<div class="spacer"></div>
	{#if !searching}
		<label class="arch row">
			<input type="checkbox" bind:checked={showArchived} /> Archived
		</label>
	{/if}
	<button
		class="btn btn-sm"
		title="Toggle compact / detailed rows"
		onclick={() => (dense = !dense)}>{dense ? '☰ Compact' : '▤ Detailed'}</button
	>
	{#if !searching}
		{#if selecting}
			<button class="btn btn-sm" onclick={exitSelect}>Cancel</button>
		{:else}
			<button
				class="btn btn-sm"
				title="Select multiple to archive"
				onclick={() => (selecting = true)}>☑ Select</button
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
			{showArchived ? 'Unarchive' : 'Archive'}
			{selected.size || ''}
		</button>
	</div>
{/if}

{#if searching}
	{#if $search.isLoading}
		<div class="empty"><span class="spin"></span></div>
	{:else if searchResults.length === 0}
		<div class="empty">No chats match “{query}”.</div>
	{:else}
		<div class="stack" class:tight={dense}>
			{#each [{ label: 'Live', rows: liveResults }, { label: 'Archived', rows: archivedResults }] as sec (sec.label)}
				{#if sec.rows.length > 0}
					<div class="group-header">
						{sec.label} <span class="count">{sec.rows.length}</span>
					</div>
					{#each sec.rows as s (s.id)}
						<SessionCard
							session={s}
							compact={dense}
							pendingCount={pending(s.id)}
							onopen={(x) => (openSession = x)}
							swipeable
							swipeLabel={s.status === 'archived' ? 'Unarchive' : 'Archive'}
							onSwipe={swipeArchive}
						/>
					{/each}
				{/if}
			{/each}
		</div>
	{/if}
{:else if $sessions.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if topLevel.length === 0}
	<div class="empty">No sessions{showArchived ? '' : ' — toggle Archived or start one'}.</div>
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
					swipeLabel={showArchived ? 'Unarchive' : 'Archive'}
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
						swipeLabel={showArchived ? 'Unarchive' : 'Archive'}
						onSwipe={swipeArchive}
					/>
				{/each}
			{/each}
		{/each}
	</div>
{/if}

{#if liveOpen}
	<ConversationDrawer session={liveOpen} onclose={() => (openSession = null)} />
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
	}
	.page-title {
		font-size: var(--fs-2xl);
	}
	.arch {
		font-size: var(--fs-sm);
		color: var(--text-muted);
		gap: var(--sp-1);
	}
	.search {
		flex: 1;
		min-width: 0;
		max-width: 22rem;
		padding: var(--sp-1) var(--sp-2);
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
