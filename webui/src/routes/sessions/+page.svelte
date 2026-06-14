<script lang="ts">
	import { untrack, onMount } from 'svelte';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { useSessions, useSessionActions, useLabels, endpoints, qk } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { page } from '$app/state';
	import { pushState, replaceState } from '$app/navigation';
	import { toasts } from '$lib/toast.svelte';
	import { ApiError } from '$lib/api';
	import { ws } from '$lib/ws.svelte';
	import SessionCard from '$lib/components/organisms/SessionCard.svelte';
	import ConversationDrawer from '$lib/components/organisms/ConversationDrawer.svelte';
	import SpawnModal from '$lib/components/organisms/SpawnModal.svelte';
	import SessionControls from '$lib/components/organisms/SessionControls.svelte';
	import { AutoGrid, Button, Container, Text } from '@dorsk/tsumikit';
	import { drafts, LIST_DENSITY, LIST_VIEW, LIST_SECTION, LIST_LABELS } from '$lib/drafts';
	import { notify } from '$lib/notify.svelte';
	import { tokenizeQuery } from '$lib/search';
	import {
		parseSections,
		PAGE,
		INLINE_THRESHOLD,
		nest,
		archivedDescendantsOf,
		costRollup,
		groupId,
		BUCKETS,
		isDispatched,
		groupOf,
		type Section,
		type SubGroup
	} from './sessions.logic';

	// Parse the persisted comma-joined label-filter ids back into a list.
	function parseLabelFilter(raw: string | null | undefined): string[] {
		return (raw ?? '').split(',').filter(Boolean);
	}

	let dense = $state(drafts.get(LIST_DENSITY) === 'compact');
	$effect(() => {
		drafts.set(LIST_DENSITY, dense ? 'compact' : 'normal');
	});

	// Main-list layout × density semantics (CCT-305). Two independent toggles:
	//   list ⇄ grid (cardView) and compact ⇄ detailed (dense). The 2×2 matrix:
	//     list  + compact  → one row per session
	//     list  + detailed → multi-row per session
	//     grid  + compact  → 2-column grid of cards kept INSIDE the centered list
	//                        container (same max-width as the rest of the UI)
	//     grid  + detailed → the container max-width is released: section headings
	//                        AND the grid span the full window; cards auto-fill up
	//                        to a max width (never narrower than a compact card)
	//                        and gain extra verticality (a taller message clamp)
	//   Card MARKUP is identical across compact/detailed in grid (always the
	//   detailed card); only the container width / column template / message clamp
	//   change. Grid is top-level only (subagents stay in list view / the drawer).
	let cardView = $state(drafts.get(LIST_VIEW) === 'card');
	$effect(() => {
		drafts.set(LIST_VIEW, cardView ? 'card' : 'list');
	});

	// View picker (CCT-307): `cardView` (list ⇄ card) and `dense` (compact ⇄
	// detailed) round-trip through drafts above; the ViewPicker molecule owns the
	// picker UI and writes back to them via bindable props.

// Section filter (CCT-322 / CCT-345): the sessions list is partitioned into
	// four sections, each an INDEPENDENT on/off toggle (not a forced single
	// choice) — one toolbar button opens a popover of four checkboxes so any
	// combination can be shown at once. Semantics over the loaded list:
	//   • starred    → pinned sessions (CCT-267)
	//   • live       → interactive, non-dispatched, non-pinned sessions
	//   • dispatched → server-managed / ephemeral-worker sessions (CCT-231)
	//   • archived   → also append the paginated archive browse (CCT-184)
	// The chosen set is persisted (comma-joined) so it sticks across reloads.
	// The SectionFilter molecule owns the popover + toggle UI and writes back the
	// chosen set via its bindable prop; the page keeps the state (and persists it).
	let sections = $state<Set<Section>>(parseSections(drafts.get(LIST_SECTION)));
	$effect(() => {
		drafts.set(LIST_SECTION, [...sections].join(','));
	});
	// `showArchived` drives the paginated archive pager + search scope; archived is
	// now just one of the enabled sections, so the existing pager wiring is reused.
	const showArchived = $derived(sections.has('archived'));
	let openSession = $state<SessionListItem | null>(null);
	let showSpawn = $state(false);
	// Prefill for "new session from same script" (CCT-250 item 8). Seeded from an
	// archived session's config, then handed to the SpawnModal.
	let spawnPrefill = $state<Record<string, string> | null>(null);
	function newFromScript(s: SessionListItem) {
		const adapter = s.adapter_id ?? 'claude-code';
		// Model is per-adapter in the spawn form (CCT-274); seed the field that
		// matches this session's adapter.
		const modelField = adapter === 'codex' ? 'model_codex' : 'model_claude';
		spawnPrefill = {
			machine_id: s.machine_id,
			working_dir: s.working_dir,
			adapter_id: adapter,
			name: '',
			[modelField]: s.model ?? ''
		};
		openSession = null;
		showSpawn = true;
	}

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
	// Seed lastUrlId from the URL on mount so a deep-link load doesn't double-push,
	// while opening a session from the list DOES push a history entry — so the
	// browser Back button (and the drawer's < button) returns to /sessions instead
	// of skipping the list (CCT-345 / CCT-326). `mounted` is reactive so the
	// drawer→URL effect re-runs once the initial sync is in place.
	let mounted = $state(false);
	onMount(() => {
		lastUrlId =
			(page.params.session as string | undefined) ?? page.url.searchParams.get('session') ?? null;
		mounted = true;
	});

	function setUrlSession(id: string | null, replace = false) {
		const url = new URL(page.url);
		url.searchParams.delete('session');
		url.pathname = id ? `/sessions/${encodeURIComponent(id)}` : '/sessions';
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
		} catch (e) {
			// Archived/ended sessions now resolve from the DB (read-only), so a
			// 404 means the session was actually DELETED — only then toast +
			// drop the id (CCT-250 item 6). Transient errors (network, 5xx) leave
			// the URL intact so a retry/refresh can recover instead of nagging.
			if (e instanceof ApiError && e.status === 404) {
				toasts.err('Session not found — it may have been deleted.');
				openSession = null;
				setUrlSession(null, true);
			} else {
				toasts.err(`Could not open session: ${(e as Error).message}`);
			}
		} finally {
			urlResolving = false;
		}
	}

	// Open a freshly forked session (CCT-345): the server pre-minted its id but
	// the DB row only appears once the daemon launches the worker and the next
	// roster poll lands (~2-3s). Poll a few times so we open it in place without
	// a manual refresh, and don't false-toast "not found" during the gap.
	async function navigateToForked(id: string) {
		for (let i = 0; i < 16; i++) {
			const found = [...items, ...pageRows].find((s) => s.id === id);
			if (found) {
				openSession = found;
				return;
			}
			try {
				openSession = await endpoints.session(id);
				return;
			} catch {
				// not registered yet — keep polling
			}
			await qc.invalidateQueries({ queryKey: qk.sessions(false) });
			await new Promise((r) => setTimeout(r, 500));
		}
		toasts.err('Forked conversation is taking a while to appear — it will show in the list shortly.');
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
		const legacyId = page.url.searchParams.get('session');
		const id = (page.params.session as string | undefined) ?? legacyId;
		if (id === untrack(() => openSession?.id ?? null)) return;
		if (id) void openById(id);
		else openSession = null;
	});

	// drawer → URL: reflect the open session into the address bar so it's always
	// a shareable link. Skip while we're resolving a URL-driven open (no echo).
	$effect(() => {
		const id = openSession?.id ?? null;
		if (urlResolving) return;
		if (!mounted) return;
		if (id === lastUrlId) return;
		// Always push so opening/closing a session is a real history step and Back
		// returns to the list. The deep-link case is handled by seeding lastUrlId
		// on mount (above), so no redundant entry is added on first load.
		setUrlSession(id, false);
		lastUrlId = id;
	});

	// Live buckets always show non-archived sessions; the archive is a separate
	// paginated section below (CCT-184).
	const sessions = useSessions(() => false);

	const qc = useQueryClient();
	const actions = useSessionActions();

	// Labels (CCT-360): the global label set feeds both the per-card picker and
	// the toolbar filter. `labelFilter` holds the selected label ids; when
	// non-empty the live list and archive browse are narrowed to sessions
	// carrying at least one of them (OR semantics). Persisted across reloads.
	const labelsQuery = useLabels();
	const allLabels = $derived($labelsQuery.data?.labels ?? []);
	// The LabelFilter molecule owns the popover + toggle UI; the page keeps the
	// selected-id set (and persists it / prunes deleted ids below).
	let labelFilter = $state(new Set<string>(parseLabelFilter(drafts.get(LIST_LABELS))));
	$effect(() => {
		drafts.set(LIST_LABELS, [...labelFilter].join(','));
	});
	// Drop filter ids whose label was deleted so the count stays honest.
	$effect(() => {
		// Wait for the label set to actually load before pruning: on a refresh
		// the persisted filter is restored synchronously while `labelsQuery` is
		// still in flight (allLabels === []), so pruning here would treat every
		// restored id as "deleted", wipe the filter, and the drafts.set effect
		// above would then persist that empty set — losing the filter for good.
		if (!$labelsQuery.data) return;
		const known = new Set(allLabels.map((l) => l.id));
		if ([...labelFilter].some((id) => !known.has(id))) {
			labelFilter = new Set([...labelFilter].filter((id) => known.has(id)));
		}
	});
	const matchesLabelFilter = (s: SessionListItem): boolean =>
		labelFilter.size === 0 || s.labels.some((l) => labelFilter.has(l.id));

	// Per-card label callbacks, threaded into every SessionCard.
	const createLabel = (name: string, color: string) => actions.createLabel(name, color);
	const attachLabel = (id: string, labelId: string) => actions.attachLabel(id, labelId);
	const detachLabel = (id: string, labelId: string) => actions.detachLabel(id, labelId);
	const updateLabel = (labelId: string, patch: { name?: string; color?: string }) =>
		actions.updateLabel(labelId, patch);
	const deleteLabel = (labelId: string) => actions.deleteLabel(labelId);

	// ── Search + archive browse (CCT-184) ──────────────────────────────────
	// One paginated "pager" feeds two views, never both at once:
	//   • searching (q non-empty) → search results, scoped by `showArchived`
	//     (unticked = live only, ticked = all). Split into Live / Archived.
	//   • not searching + showArchived → browse the archive (empty q), paged.
	// Live-only with no query needs no pager — the bucketed list owns it.
	// `rawQuery` is the live input (debounced into `query`); the SearchBox molecule
	// binds to it and owns the field UI (clear button, focus).
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

	// Bulk-archive every Dispatched conversation at once (CCT-279 item 7). Uses
	// the existing batch endpoint (POST /sessions/archive) over all dispatched
	// (server-managed machine) sessions in the live list, children included.
	async function archiveAllDispatched() {
		const ids = items.filter(isDispatched).map((s) => s.id);
		if (ids.length === 0) return;
		if (!confirm(`Archive all ${ids.length} dispatched conversation${ids.length === 1 ? '' : 's'}?`))
			return;
		archiving = true;
		try {
			await actions.archiveMany(ids);
			toasts.ok(`Archived ${ids.length}`);
			refreshTick++;
			qc.invalidateQueries({ queryKey: ['sessions'] });
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

	// Pin/unpin a session (CCT-267). Pinning floats it to the top group and
	// exempts it from auto-archive; the list refetches so the move is immediate.
	async function togglePin(s: SessionListItem) {
		try {
			if (s.pinned) await actions.unpin(s.id);
			else await actions.pin(s.id);
			toasts.ok(s.pinned ? 'Unpinned' : 'Pinned');
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

	// A starred parent should keep its full subagent group visible under Pinned
	// even once the children are archived (CCT-297): the live list above excludes
	// archived rows, so when anything is pinned we additionally pull the full
	// (incl. archived) list and splice each pinned parent's archived descendants
	// back into the nest. Gated on `pinnedIds.size` so the heavier full-list
	// fetch only runs when there's actually a pin in play.
	const pinnedIds = $derived(new Set(items.filter((s) => s.pinned).map((s) => s.id)));
	const allSessions = useSessions(
		() => true,
		() => pinnedIds.size > 0
	);
	const archivedPool = $derived(
		($allSessions.data?.sessions ?? []).filter((s) => s.status === 'archived')
	);
	const pinnedArchivedKids = $derived(archivedDescendantsOf(pinnedIds, archivedPool));
	// Their ids, so the Archived browse below doesn't also list them as their own
	// top-level rows — they already show nested under their pinned parent.
	const pinnedArchivedKidIds = $derived(new Set(pinnedArchivedKids.map((s) => s.id)));

	// Subagent grouping (CCT-225 / CCT-269), nesting (CCT-298 item 1), and the
	// cost rollup (CCT-297 #19) are all pure data transforms — see
	// sessions.logic.ts. The component keeps only the reactive derivations + the
	// expand/collapse state below.
	const liveNest = $derived(nest([...items, ...pinnedArchivedKids]));
	const topLevel = $derived(liveNest.topLevel);
	const childGroupsOf = $derived(liveNest.childGroups);

	// Expand/collapse state for collapsible (>=3) subagent groups, keyed by
	// `${parentId}/${group.key}`. Default collapsed.
	let expanded = $state(new Set<string>());
	function toggleGroup(parentId: string, key: string) {
		const id = groupId(parentId, key);
		const next = new Set(expanded);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		expanded = next;
	}

	// Classifier buckets (CCT-90) + the dispatch/pinned mapping are pure — see
	// BUCKETS / isDispatched / groupOf in sessions.logic.ts.
	type GroupKey = ReturnType<typeof groupOf>;
	// Each live bucket maps to exactly ONE section toggle, so the four toggles
	// select disjoint slices that compose cleanly (CCT-345):
	//   • Pinned bucket      ← starred
	//   • Dispatched bucket  ← dispatched
	//   • every other bucket ← live
	// (Archived isn't a bucket — it appends the paginated archive browse below.)
	const bucketInSection = (key: GroupKey): boolean => {
		if (key === 'pinned') return sections.has('starred');
		if (key === 'dispatched') return sections.has('dispatched');
		return sections.has('live');
	};
	const groups = $derived(
		BUCKETS.filter((b) => bucketInSection(b.key)).map((b) => ({
			...b,
			sessions: topLevel.filter((s) => groupOf(s) === b.key && matchesLabelFilter(s))
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

<SessionControls
	bind:rawQuery
	bind:sections
	labels={allLabels}
	bind:labelFilter
	bind:cardView
	bind:dense
	{selecting}
	{searching}
	onStartSelect={() => (selecting = true)}
	onCancelSelect={exitSelect}
	onNew={() => (showSpawn = true)}
	onUpdateLabel={updateLabel}
	onDeleteLabel={deleteLabel}
/>

{#if selecting && !searching}
		<div class="bulkbar row">
			<Text class="count" size="sm" weight="semibold" tone="muted">{selected.size} selected</Text>
			<Button size="sm" onclick={selectAll}>Select all</Button>
			<div class="spacer"></div>
			<Button
				size="sm"
				variant="danger"
				disabled={selected.size === 0 || archiving}
				onclick={archiveSelected}
			>
				{#if archiving}<span class="spin"></span>{/if}
				Archive {selected.size || ''}
			</Button>
		</div>
{/if}

<!-- Nested list of top-level rows with subagent count badges + inline children,
     shared by the live buckets, search results, and the archive browse so they
     all render the same nesting (CCT-298 item 1). `allowSelect` gates the
     multi-select checkboxes (live buckets only); `hl` carries search terms. -->
<!-- Card (grid) view of the main list (CCT-297 item 16): top-level sessions laid
     out as detailed cards in a responsive grid. Subagents are omitted here (the
     list view + the drawer still show them); the point is at-a-glance status. -->
{#snippet cardItems(rows: SessionListItem[])}
	{#each rows as s (s.id)}
		<SessionCard
			session={s}
			compact={dense}
			grid
			pendingCount={pending(s.id)}
			onopen={(x) => (openSession = x)}
			selectable={selecting}
			selected={selected.has(s.id)}
			onToggleSelect={toggleSelect}
			swipeable
			swipeLabel="Archive"
			onSwipe={swipeArchive}
			onTogglePin={togglePin}
			subagentCost={costRollup(s, childGroupsOf.get(s.id) ?? [])}
			{allLabels}
			onCreateLabel={createLabel}
			onAttachLabel={attachLabel}
			onDetachLabel={detachLabel}
			onUpdateLabel={updateLabel}
			onDeleteLabel={deleteLabel}
		/>
	{/each}
{/snippet}

{#snippet cardGrid(rows: SessionListItem[])}
	{#if dense}
		<AutoGrid min="18rem" max="26.75rem" maxCols={2} gap="var(--sp-2)">{@render cardItems(rows)}</AutoGrid>
	{:else}
		<AutoGrid min="20rem" max="26.75rem" gap="var(--sp-3)">{@render cardItems(rows)}</AutoGrid>
	{/if}
{/snippet}

{#snippet nestedRows(
	rows: SessionListItem[],
	childGroups: Map<string, SubGroup[]>,
	allowSelect: boolean,
	hl: string[],
	depth = 0
)}
	{#each rows as s (s.id)}
		{@const subGroups = childGroups.get(s.id) ?? []}
		{@const collapsibleGroups = subGroups.filter((g) => g.agents.length >= INLINE_THRESHOLD)}
		<!-- Collapsible (>=3) groups surface as count badges outside the parent
		     row layout; smaller groups render inline below. -->
		<div class="parent-row" class:dense>
			<SessionCard
				session={s}
				child={depth > 0}
				compact={dense}
				pendingCount={pending(s.id)}
				onopen={(x) => (openSession = x)}
				selectable={allowSelect && selecting}
				selected={selected.has(s.id)}
				onToggleSelect={toggleSelect}
				swipeable
				swipeLabel={s.status === 'archived' ? 'Unarchive' : 'Archive'}
				onSwipe={swipeArchive}
				onTogglePin={depth > 0 ? undefined : togglePin}
				highlight={hl}
				subagentCost={costRollup(s, subGroups)}
				subagentToggles={collapsibleGroups.map((g) => ({
					key: g.key,
					count: g.agents.length,
					running: g.running,
					open: expanded.has(groupId(s.id, g.key)),
					label: g.label,
					ontoggle: () => toggleGroup(s.id, g.key)
				}))}
				{allLabels}
				onCreateLabel={createLabel}
				onAttachLabel={depth > 0 ? undefined : attachLabel}
				onDetachLabel={depth > 0 ? undefined : detachLabel}
				onUpdateLabel={updateLabel}
				onDeleteLabel={deleteLabel}
			/>
		</div>
		{#if depth < 5}
			{#each subGroups as g (g.key)}
				{#if g.agents.length < INLINE_THRESHOLD || expanded.has(groupId(s.id, g.key))}
					<div class="agent-children" style="--agent-depth: {Math.min(depth + 1, 5)}">
						{@render nestedRows(g.agents, childGroups, allowSelect, hl, depth + 1)}
					</div>
				{/if}
			{/each}
		{/if}
	{/each}
{/snippet}

{#snippet loadMore()}
	{#if pageError}
		<div class="empty err"><Text tone="danger">Search failed: {pageError}</Text></div>
	{:else if pageLoading}
		<div class="loadmore"><span class="spin"></span></div>
		{:else if !pageDone && pageRows.length > 0}
			<div class="loadmore">
				<Button size="sm" onclick={() => loadPage(false)}>Load more</Button>
			</div>
		{/if}
{/snippet}

{#if searching}
	<!-- Search results, scoped by the Archived checkbox; split Live / Archived. -->
	{#if pageLoading && pageRows.length === 0}
		<div class="empty"><span class="spin"></span></div>
	{:else if pageRows.length === 0}
		<div class="empty"><Text tone="muted">No chats match “{query}”{showArchived ? '' : ' (live only — pick Archived to search all)'}.</Text></div>
	{:else}
		<!-- Nest over the whole result set so a parent and its subagents stay
		     grouped even if they land in different status sections; then split
		     the top-level rows into Live / Archived (CCT-298 item 1). -->
		{@const ns = nest(pageRows)}
		{@const liveTop = ns.topLevel.filter((s) => s.status !== 'archived')}
		{@const archTop = ns.topLevel.filter((s) => s.status === 'archived')}
		<div class="sections" class:tight={dense}>
			{#if liveTop.length > 0}
				<div class="section">
					<div class="group-header">Live <Text class="count">{liveTop.length}</Text></div>
					{@render nestedRows(liveTop, ns.childGroups, false, searchTerms)}
				</div>
			{/if}
			{#if archTop.length > 0}
				<div class="section">
					<div class="group-header">Archived <Text class="count">{archTop.length}</Text></div>
					{@render nestedRows(archTop, ns.childGroups, false, searchTerms)}
				</div>
			{/if}
			{@render loadMore()}
		</div>
	{/if}
{:else}
	<!-- Live buckets first, then the paginated archive — all sections share one
	     flex container so the inter-section gap is uniform (CCT-298). -->
	{#if cardView && !dense}
		<Container fullWidth as="div">
			<div class="sections">{@render liveSections()}</div>
		</Container>
	{:else}
		<div class="sections" class:tight={dense && !cardView}>{@render liveSections()}</div>
	{/if}
{/if}

{#snippet liveSections()}
		{#if $sessions.isLoading}
			<div class="empty"><span class="spin"></span></div>
		{:else if groups.length === 0 && !showArchived}
			<div class="empty">
				<Text tone="muted">No sessions in the selected sections — toggle more from the section filter.</Text>
			</div>
		{:else}
			{#each groups as g (g.key)}
				{@const vis = g.sessions}
				<div class="section">
					{#if g.key === 'dispatched'}
						<!-- Dispatched is a plain section header like Pinned/Completed, with a
						     bulk "Archive all" action on the right (CCT-279 item 7). -->
						<div class="group-header" data-bucket={g.key}>
							{g.label} <Text class="count">{g.sessions.length}</Text>
							<!-- In card mode the action sits right next to the title; in
							     list mode it's pushed to the far right via the spacer. -->
							{#if !cardView}<div class="spacer"></div>{/if}
							<Button
								size="sm"
								variant="danger"
								disabled={archiving}
								title="Archive all dispatched conversations"
								onclick={archiveAllDispatched}
							>
								{#if archiving}<span class="spin"></span>{/if}
								Archive all
							</Button>
						</div>
					{:else}
						<div class="group-header" data-bucket={g.key}>
							{g.label} <Text class="count">{g.sessions.length}</Text>
						</div>
					{/if}
					{#if cardView}
						{@render cardGrid(vis)}
					{:else}
						{@render nestedRows(vis, childGroupsOf, true, [])}
					{/if}
				</div>
			{/each}
		{/if}

		{#if showArchived}
			{@const ns = nest(pageRows)}
			{@const archTop = ns.topLevel.filter(
				(s) => matchesLabelFilter(s) && !pinnedArchivedKidIds.has(s.id)
			)}
			<div class="section">
				<div class="group-header">Archived <Text class="count">{archTop.length}</Text></div>
				{#if pageRows.length === 0 && !pageLoading}
					<div class="empty"><Text tone="muted">No archived sessions.</Text></div>
				{:else if cardView}
					<!-- Card mode applies to archived sessions too (CCT-321 parity). -->
					{@render cardGrid(archTop)}
					{@render loadMore()}
				{:else}
					{@render nestedRows(archTop, ns.childGroups, false, searchTerms)}
					{@render loadMore()}
				{/if}
			</div>
		{/if}
{/snippet}

{#if liveOpen}
	<ConversationDrawer
		session={liveOpen}
		onclose={() => (openSession = null)}
		highlight={searchTerms}
		onNewFromScript={newFromScript}
		onNavigate={(sid) => void navigateToForked(sid)}
	/>
{/if}

{#if showSpawn}
	<SpawnModal
		prefill={spawnPrefill}
		onclose={() => {
			showSpawn = false;
			spawnPrefill = null;
		}}
		onspawned={() => qc.invalidateQueries({ queryKey: ['sessions'] })}
	/>
{/if}

<style>
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
	/* Two-axis spacing (CCT-298): the outer container owns the inter-section gap;
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
	/* Parent row (CCT-269): a normal full-width row. The collapse toggle badge(s)
	   now live inside the card's leading gutter slot (SessionCard), so there's no
	   external rail and no reserved left gutter to keep aligned across sections. */
	.parent-row {
		position: relative;
	}
	.agent-children {
		margin-left: min(calc(var(--agent-depth) * var(--sp-4)), 5rem);
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	@media (max-width: 639px) {
		.agent-children {
			margin-left: min(calc(var(--agent-depth) * var(--sp-2)), 2.5rem);
		}
	}
	.loadmore {
		display: flex;
		justify-content: center;
		padding: var(--sp-3) 0;
	}
	.group-header {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		font-size: var(--fs-sm);
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--text-muted);
	}
	/* The count is a Text atom; target the passed class via :global. */
	.group-header :global(.count) {
		font-weight: 400;
		opacity: 0.7;
	}
	/* Dispatched group collapse toggle (CCT-279 item 6). */
	.group-header[data-bucket='blocked'] {
		color: var(--warn, #d08770);
	}
	.group-header[data-bucket='review'] {
		color: var(--accent, #88c0d0);
	}
</style>
