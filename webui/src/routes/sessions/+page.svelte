<script lang="ts">
	import { untrack, onMount, type Snippet } from 'svelte';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { useSessions, useSessionActions, useLabels, endpoints, qk } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { page } from '$app/state';
	import { pushState, replaceState } from '$app/navigation';
	import { toasts, type ToastAction } from '$lib/toast.svelte';
	import { ApiError, errMessage } from '$lib/api';
	import { ws } from '$lib/ws.svelte';
	import SessionCard from '$lib/components/organisms/SessionCard.svelte';
	import ConversationDrawer from '$lib/components/organisms/ConversationDrawer.svelte';
	import SpawnModal from '$lib/components/organisms/SpawnModal.svelte';
	import { dockLayout } from '$lib/spawnDock.svelte';
	import StatsDock from '$lib/components/organisms/statsdock/StatsDock.svelte';
	import SessionControls from '$lib/components/organisms/SessionControls.svelte';
	import KanbanBoard from '$lib/components/organisms/KanbanBoard.svelte';
	import { AutoGrid, Button, IconButton, Text } from '@dorsk/tsumikit';
	import { drafts, LIST_DENSITY, LIST_VIEW, LIST_KANBAN, LIST_SECTION, LIST_LABELS } from '$lib/drafts';
	import { notify } from '$lib/notify.svelte';
	import { settings } from '$lib/settings.svelte';
	import { tokenizeQuery } from '$lib/search';
	import { m } from '$lib/paraglide/messages';
	import { freeText, parse } from '@dorsk/tsumikit';
	import {
		buildSessionSearchSchema,
		contextForField,
		matchesClientFilters,
		splitQuery
	} from '$lib/searchSchema';
	import {
		parseSections,
		PAGE,
		INLINE_THRESHOLD,
		nest,
		archivedDescendantsOf,
		costRollup,
		groupId,
		isDispatched,
		inEnabledSections,
		matchesUnreadFilter,
		parseLabelFilter,
		colorHueOf,
		pickFreshSession,
		sessionIdFromLocation,
		sessionHrefFor,
		scriptPrefill,
		draftEditPrefill,
		draftPromptPreview,
		type Section,
		type SubGroup,
		type Dimension
	} from './sessions.logic';
	import { SessionsListController } from './SessionsListController.svelte';

	let dense = $state(drafts.get(LIST_DENSITY) === 'compact');
	$effect(() => {
		drafts.set(LIST_DENSITY, dense ? 'compact' : 'normal');
	});

	// Main-list layout × density semantics. Two independent toggles:
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

	// Kanban board view: a distinct layout persisted alongside the
	// list/card × density picker; when set it overrides the other two.
	let kanban = $state(drafts.get(LIST_KANBAN) === '1');
	$effect(() => {
		drafts.set(LIST_KANBAN, kanban ? '1' : '');
	});

	// Color-by and group-by dimensions, read live from the
	// server-persisted settings blob (so an async settings.load() reflows the UI)
	// and written back through settings.setSessionList (localStorage + debounced PUT).
	const colorBy = $derived(settings.state.sessionList.colorBy as Dimension);
	const groupBy = $derived(settings.state.sessionList.groupBy as Dimension);
	const accentOf = (s: SessionListItem) => colorHueOf(s, colorBy);

	// View picker: `cardView` (list ⇄ card) and `dense` (compact ⇄
	// detailed) round-trip through drafts above; the ViewPicker molecule owns the
	// picker UI and writes back to them via bindable props.

// Section filter: the sessions list is partitioned into
	// four sections, each an INDEPENDENT on/off toggle (not a forced single
	// choice) — one toolbar button opens a popover of four checkboxes so any
	// combination can be shown at once. Semantics over the loaded list:
	//   • starred    → pinned sessions
	//   • live       → interactive, non-dispatched, non-pinned sessions
	//   • dispatched → server-managed / ephemeral-worker sessions
	//   • archived   → also append the paginated archive browse
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
	// One-click "hide archived" from the Archived header itself, so turning
	// the section back off doesn't cost a trip through the filter popover.
	// Mirrors SectionFilter's invariant: the set never goes empty, so when
	// Archived was the only section on we fall back to Live.
	function hideArchived() {
		const next = new Set(sections);
		next.delete('archived');
		if (next.size === 0) next.add('live');
		sections = next;
	}
	let openSession = $state<SessionListItem | null>(null);
	let showSpawn = $state(false);
	// Docked panels (Settings › New session / Stats panel): the spawn form
	// stays pinned to one edge instead of living behind the "+ New" button
	// (`spawn` null = modal mode), and the stats panel to the same or the other.
	const docks = $derived(dockLayout());
	const dockSide = $derived(docks.spawn);
	// Bumped whenever the docked form is done with (spawned, drafted, cleared,
	// or handed a prefill) so it remounts and reseeds exactly like a reopened
	// modal would.
	let dockEpoch = $state(0);
	// Prefill for "new session from same script". Seeded from an
	// archived session's config, then handed to the SpawnModal.
	let spawnPrefill = $state<Record<string, string> | null>(null);
	// Open the spawn form seeded with `prefill`: the modal, or a fresh mount of
	// the docked panel.
	function openSpawn(prefill: Record<string, string> | null) {
		spawnPrefill = prefill;
		if (dockSide) dockEpoch++;
		else showSpawn = true;
	}
	function newFromScript(s: SessionListItem) {
		openSession = null;
		openSpawn(scriptPrefill(s));
	}

	// ── Deep-linkable session ─────────────────────────────────────
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
	// of skipping the list. `mounted` is reactive so the
	// drawer→URL effect re-runs once the initial sync is in place.
	let mounted = $state(false);
	// Derive the session id from the URL pathname rather than `page.params`.
	// `setUrlSession` navigates with shallow routing (pushState/replaceState),
	// which updates `page.url` but does NOT re-resolve the matched route — so
	// `page.params.session` stays pinned to whatever the [session] route bound on
	// the last full navigation. After closing the drawer pushes `/sessions`, the
	// stale param would still read `<uuid>`, reopen the session, and re-push
	// the URL (the back chevron never clearing /sessions/<uuid>). Parsing the
	// live pathname keeps the URL→drawer effect honest under shallow routing.
	const sessionIdFromUrl = (): string | null =>
		sessionIdFromLocation(page.url.pathname, page.url.searchParams);
	onMount(() => {
		lastUrlId = sessionIdFromUrl();
		mounted = true;
	});

	function setUrlSession(id: string | null, replace = false) {
		const href = sessionHrefFor(location.href, id);
		if (href === null) return;
		if (replace) replaceState(href, {});
		else pushState(href, {});
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
			// drop the id. Transient errors (network, 5xx) leave
			// the URL intact so a retry/refresh can recover instead of nagging.
			if (e instanceof ApiError && e.status === 404) {
				toasts.err(m.sessions_toast_not_found());
				openSession = null;
				setUrlSession(null, true);
			} else {
				toasts.err(m.sessions_toast_open_failed({ error: errMessage(e) }));
			}
		} finally {
			urlResolving = false;
		}
	}

	// Open a freshly forked session: the server pre-minted its id but
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
		toasts.err(m.sessions_toast_fork_slow());
	}

	// URL → drawer: react to the `session` param (initial load, back/forward,
	// pasted link). Only act when it differs from what's already open. The
	// `openSession` read is untracked: if this effect depended on it,
	// any `openSession = …` (card click, notification) would re-run it *before*
	// the drawer→URL effect below pushes `?session=<id>` — the still-empty URL
	// param then hit the `openSession = null` branch and closed the drawer in
	// the same flush, so conversations never opened. Depending only on the URL
	// keeps this effect to its job: URL changes drive the drawer, not vice versa.
	$effect(() => {
		const id = sessionIdFromUrl();
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
	// paginated section below.
	const sessions = useSessions(() => false);

	const qc = useQueryClient();
	const actions = useSessionActions();

	// Labels: the global label set feeds both the per-card picker and
	// the toolbar filter. `labelFilter` holds the selected label ids; when
	// non-empty the live list and archive browse are narrowed to sessions
	// carrying at least one of them (OR semantics). Persisted across reloads.
	const labelsQuery = useLabels();
	const allLabels = $derived(labelsQuery.data?.labels ?? []);
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
		if (!labelsQuery.data) return;
		const known = new Set(allLabels.map((l) => l.id));
		if ([...labelFilter].some((id) => !known.has(id))) {
			labelFilter = new Set([...labelFilter].filter((id) => known.has(id)));
		}
	});
	const matchesLabelFilter = (s: SessionListItem): boolean =>
		labelFilter.size === 0 || s.labels.some((l) => labelFilter.has(l.id));

	// One predicate for search results AND the archive browse so the two
	// branches can't drift apart on which filters apply.
	const keepRow = (s: SessionListItem): boolean =>
		inEnabledSections(s, sections) &&
		matchesLabelFilter(s) &&
		matchesClient(s) &&
		matchesUnreadFilter(s, sections);

	// Per-card label callbacks, threaded into every SessionCard.
	const createLabel = (name: string, color: string) => actions.createLabel(name, color);
	const attachLabel = (id: string, labelId: string) => actions.attachLabel(id, labelId);
	const detachLabel = (id: string, labelId: string) => actions.detachLabel(id, labelId);
	const updateLabel = (labelId: string, patch: { name?: string; color?: string }) =>
		actions.updateLabel(labelId, patch);
	const deleteLabel = (labelId: string) => actions.deleteLabel(labelId);

	// ── Search + archive browse ──────────────────────────────────
	// One paginated "pager" feeds two views, never both at once:
	//   • searching (q non-empty) → search results, scoped by `showArchived`
	//     (unticked = live only, ticked = all). Split into Live / Archived.
	//   • not searching + showArchived → browse the archive (empty q), paged.
	// Live-only with no query needs no pager — the bucketed list owns it.
	// `rawQuery` is the live input (debounced into `query`); the FilterSearchBar
	// organism binds to it and owns the field UI (chips, autocomplete, clear).
	let rawQuery = $state('');
	const searchSchema = buildSessionSearchSchema((field, q) =>
		endpoints.searchFieldValues(field, q, contextForField(rawQuery, searchSchema, field))
	);
	let query = $state('');
	$effect(() => {
		const v = rawQuery.trim();
		const t = setTimeout(() => (query = v), 200);
		return () => clearTimeout(t);
	});
	// Server-evaluable clauses + free text ride the raw string to the search
	// endpoint; `id`/`created` clauses are peeled off and narrowed client-side.
	const split = $derived(splitQuery(query, searchSchema));
	const serverQuery = $derived(split.serverQuery);
	const clientFilters = $derived(split.clientFilters);
	const searching = $derived(serverQuery.length > 0);
	const pagerActive = $derived(searching || showArchived);
	const matchesClient = (s: SessionListItem): boolean =>
		matchesClientFilters(s, clientFilters);
	// Highlight only the free-text portion of the query (field clauses excluded).
	const searchTerms = $derived(tokenizeQuery(freeText(parse(query, searchSchema))));

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
				serverQuery,
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
			if (req === pageReqId) pageError = errMessage(e);
		} finally {
			if (req === pageReqId) pageLoading = false;
		}
	}

	// Reset + reload page 0 whenever the mode (query / scope) changes, or an
	// archive action bumps refreshTick.
	const pagerKey = $derived(`${searching ? `q:${serverQuery}` : 'browse'}|${showArchived}`);
	$effect(() => {
		void pagerKey;
		void refreshTick;
		pageRows = [];
		pageOffset = 0;
		pageDone = false;
		pageError = '';
		if (pagerActive) loadPage(true);
	});

	// Multi-select (selecting / selected / anchor) + subagent-group expand state
	// and their transitions live on the controller; the batch archive that calls
	// the server stays here.
	let archiving = $state(false);

	// Visual order of the list, read from the DOM: rows are rendered by a
	// recursive snippet across several buckets, so document order is the only
	// place the flattened order the user actually sees exists.
	function renderedIds(): string[] {
		return [...document.querySelectorAll<HTMLElement>('[data-session-id]')]
			.map((el) => el.dataset.sessionId!)
			.filter((id) => id);
	}

	// "Undo" action attached to every archive toast: un-archives the same ids
	// and refreshes the list. Only reachable while the toast is still visible.
	function undoArchive(ids: string[]): ToastAction {
		return {
			label: m.toast_undo(),
			run: async () => {
				await actions.unarchiveMany(ids);
				toasts.ok(m.sessions_toast_unarchived());
				refreshTick++;
				qc.invalidateQueries({ queryKey: ['sessions'] });
			}
		};
	}

	async function archiveSelected() {
		const ids = [...list.selected];
		if (ids.length === 0) return;
		if (ids.length > 1 && !confirm(m.sessions_confirm_archive_many({ count: ids.length }))) return;
		archiving = true;
		try {
			await actions.archiveMany(ids);
			toasts.ok(m.sessions_toast_archived({ count: ids.length }), undoArchive(ids));
			list.exitSelect();
			refreshTick++;
		} catch (e) {
			toasts.err(errMessage(e));
		} finally {
			archiving = false;
		}
	}

	// Bulk-archive every Dispatched conversation at once. Uses
	// the existing batch endpoint (POST /sessions/archive) over all dispatched
	// (server-managed machine) sessions in the live list, children included.
	async function archiveAllDispatched() {
		const ids = items.filter(isDispatched).map((s) => s.id);
		if (ids.length === 0) return;
		if (!confirm(m.sessions_confirm_archive_all_dispatched({ count: ids.length })))
			return;
		archiving = true;
		try {
			await actions.archiveMany(ids);
			toasts.ok(m.sessions_toast_archived({ count: ids.length }), undoArchive(ids));
			refreshTick++;
			qc.invalidateQueries({ queryKey: ['sessions'] });
		} catch (e) {
			toasts.err(errMessage(e));
		} finally {
			archiving = false;
		}
	}

	// Swipe-to-archive a single row. Status-aware so it works for both
	// live (archive) and archived (unarchive) rows.
	async function swipeArchive(s: SessionListItem) {
		const isArchived = s.status === 'archived';
		try {
			if (isArchived) await actions.unarchive(s.id);
			else await actions.archive(s.id);
			toasts.ok(
				isArchived ? m.sessions_toast_unarchived() : m.sessions_toast_archived_one(),
				isArchived ? undefined : undoArchive([s.id])
			);
			refreshTick++;
		} catch (e) {
			toasts.err(errMessage(e));
		}
	}

	// Pin/unpin a session. Pinning floats it to the top group and
	// exempts it from auto-archive; the list refetches so the move is immediate.
	async function togglePin(s: SessionListItem) {
		try {
			if (s.pinned) await actions.unpin(s.id);
			else await actions.pin(s.id);
			toasts.ok(s.pinned ? m.sessions_toast_unpinned() : m.sessions_toast_pinned());
			refreshTick++;
		} catch (e) {
			toasts.err(errMessage(e));
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

	// Unread tracking: mark the open session's messages seen server-side,
	// then refetch so its badge drops to zero. `changeTick` re-runs it as new
	// messages stream in while the drawer stays open, keeping the open session at
	// zero instead of re-accumulating unread.
	$effect(() => {
		void ws.changeTick;
		const id = openSession?.id ?? null;
		if (!id) return;
		void actions
			.markSeen(id)
			.then(() => qc.invalidateQueries({ queryKey: ['sessions'] }))
			.catch(() => {});
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

	const items = $derived(sessions.data?.sessions ?? []);

	// Draft/staged sessions (status='draft') are pulled out of the classifier
	// buckets into their own section by the controller (list.draftRows).
	let launchingDraft = $state<string | null>(null);

	// Launch a draft: the server mints account env fresh at dispatch and removes
	// the draft row; the live session appears via the daemon's registration.
	async function launchDraft(s: SessionListItem) {
		launchingDraft = s.id;
		try {
			await actions.launchDraft(s.id);
			toasts.ok(m.sessions_toast_draft_launched());
		} catch (e) {
			toasts.err(m.sessions_toast_launch_failed({ error: errMessage(e) }));
		} finally {
			launchingDraft = null;
		}
	}

	async function discardDraft(s: SessionListItem) {
		if (!confirm(m.sessions_confirm_discard_draft())) return;
		try {
			await actions.discardDraft(s.id);
			toasts.ok(m.sessions_toast_draft_discarded());
		} catch (e) {
			toasts.err(errMessage(e));
		}
	}

	// Edit a draft: discard it and reopen the spawn modal prefilled from its
	// stored config, so saving/launching from there replaces it (no duplicate).
	async function editDraft(s: SessionListItem) {
		const prefill = draftEditPrefill(s);
		try {
			await actions.discardDraft(s.id);
		} catch (e) {
			toasts.err(m.sessions_toast_edit_draft_failed({ error: errMessage(e) }));
			return;
		}
		openSpawn(prefill);
	}

	// A starred parent should keep its full subagent group visible under Pinned
	// even once the children are archived: the live list above excludes
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
		(allSessions.data?.sessions ?? []).filter((s) => s.status === 'archived')
	);
	const pinnedArchivedKids = $derived(archivedDescendantsOf(pinnedIds, archivedPool));
	// Their ids, so the Archived browse below doesn't also list them as their own
	// top-level rows — they already show nested under their pinned parent.
	const pinnedArchivedKidIds = $derived(new Set(pinnedArchivedKids.map((s) => s.id)));

	// The list derivations (nest/buckets/group-by/kanban) + multi-select and
	// subagent-group expand state live on the controller; the component supplies
	// the reactive inputs and delegates. Subagent nesting and the cost rollup are
	// pure data transforms — see sessions.logic.ts.
	const list = new SessionsListController({
		items: () => items,
		pinnedArchivedKids: () => pinnedArchivedKids,
		sections: () => sections,
		groupBy: () => groupBy,
		sort: () => settings.state.sessionList.sort,
		matchesLabel: matchesLabelFilter,
		matchesClient,
		renderedOrder: renderedIds
	});
	const childGroupsOf = $derived(list.childGroupsOf);

	const pending = (id: string) => {
		void ws.changeTick; // re-derive when perms change (setPerms bumps changeTick)
		return ws.pendingCount(id);
	};

	// keep the open drawer's session object fresh as the list refetches
	const liveOpen = $derived(pickFreshSession(openSession, [...items, ...pageRows]));
</script>

<SessionControls
	bind:rawQuery
	{searchSchema}
	bind:sections
	labels={allLabels}
	bind:labelFilter
	bind:cardView
	bind:dense
	bind:kanban
	{colorBy}
	{groupBy}
	onColorBy={(v) => settings.setSessionList({ colorBy: v })}
	onGroupBy={(v) => settings.setSessionList({ groupBy: v })}
	selecting={list.selecting}
	{searching}
	onStartSelect={() => (list.selecting = true)}
	onCancelSelect={list.exitSelect}
	onNew={dockSide ? undefined : () => (showSpawn = true)}
	onUpdateLabel={updateLabel}
	onDeleteLabel={deleteLabel}
/>

{#if list.selecting}
		<div class="bulkbar row">
			<Text class="count" size="sm" weight="semibold" tone="muted">{m.sessions_selected_count({ count: list.selected.size })}</Text>
			<Button onclick={list.selectAll}>{m.sessions_select_all()}</Button>
			<Text size="xs" tone="muted">{m.sessions_select_range_hint()}</Text>
			<div class="spacer"></div>
			<Button
				variant="danger"
				disabled={list.selected.size === 0 || archiving}
				onclick={archiveSelected}
			>
				{#if archiving}<span class="spin"></span>{/if}
				{m.sessions_archive_count({ count: list.selected.size || '' })}
			</Button>
		</div>
{/if}

<!-- Nested list of top-level rows with subagent count badges + inline children,
     shared by the live buckets, search results, and the archive browse so they
     all render the same nesting. `allowSelect` gates the
     multi-select checkboxes (live buckets only); `hl` carries search terms. -->
<!-- Card (grid) view of the main list: top-level sessions laid
     out as detailed cards in a responsive grid. Subagents are omitted here (the
     list view + the drawer still show them); the point is at-a-glance status. -->
{#snippet cardItems(
	rows: SessionListItem[],
	childGroups: Map<string, SubGroup[]>,
	hl: string[] = [],
	depth = 0
)}
	{#each rows as s (s.id)}
		{@const subGroups = childGroups.get(s.id) ?? []}
		<SessionCard
			session={s}
			child={depth > 0}
			compact={dense}
			grid
			accentHue={accentOf(s)}
			stacked={subGroups.length > 0}
			pendingCount={pending(s.id)}
			unreadCount={openSession?.id === s.id ? 0 : (s.unread_count ?? 0)}
			onopen={(x) => (openSession = x)}
			selectable={list.selecting}
			selected={list.selected.has(s.id)}
			onToggleSelect={list.toggleSelect}
			swipeable
			swipeLabel={m.sessions_archive()}
			onSwipe={swipeArchive}
			onTogglePin={depth > 0 ? undefined : togglePin}
			highlight={hl}
			subagentCost={costRollup(s, subGroups)}
			subagentToggles={subGroups.map((g) => ({
				key: g.key,
				count: g.agents.length,
				running: g.running,
				open: list.expanded.has(groupId(s.id, g.key)),
				label: g.label,
				ontoggle: () => list.toggleGroup(s.id, g.key)
			}))}
			{allLabels}
			onCreateLabel={createLabel}
			onAttachLabel={depth > 0 ? undefined : attachLabel}
			onDetachLabel={depth > 0 ? undefined : detachLabel}
			onUpdateLabel={updateLabel}
			onDeleteLabel={deleteLabel}
		/>
		<!-- Clicking a count badge expands that subagent group as cards inserted
		     right after the parent card in the grid flow. -->
		{#if depth < 5}
			{#each subGroups as g (g.key)}
				{#if list.expanded.has(groupId(s.id, g.key))}
					{@render cardItems(g.agents, childGroups, hl, depth + 1)}
				{/if}
			{/each}
		{/if}
	{/each}
{/snippet}

{#snippet cardGrid(rows: SessionListItem[], childGroups: Map<string, SubGroup[]>, hl: string[] = [])}
	<!-- Card track widths scale with the UI font: the chrome stays rem-pinned
	     but card TEXT (working-dir chip, token readout, model) grows with --fs-scale, so
	     at the largest scale fixed-rem cards overflowed. Multiplying min/max by the same
	     factor widens cards as the text grows — and raising `min` naturally collapses to
	     a single compact column / wider detailed cards at high zoom. -->
	{#if dense}
		<AutoGrid min="calc(18rem * var(--fs-scale))" max="calc(26.75rem * var(--fs-scale))" maxCols={2} gap="var(--sp-2)">{@render cardItems(rows, childGroups, hl)}</AutoGrid>
	{:else}
		<AutoGrid min="calc(20rem * var(--fs-scale))" max="calc(26.75rem * var(--fs-scale))" gap="var(--sp-3)">{@render cardItems(rows, childGroups, hl)}</AutoGrid>
	{/if}
{/snippet}

<!-- Every row set — live buckets, search results, archive browse — renders
     through this one card-vs-list dispatch so no branch can drift from the
     view picker. Kanban implies cardView, so search/archive under kanban fall
     back to the card grid. -->
{#snippet rowsView(
	rows: SessionListItem[],
	childGroups: Map<string, SubGroup[]>,
	allowSelect: boolean,
	hl: string[]
)}
	{#if cardView}
		{@render cardGrid(rows, childGroups, hl)}
	{:else}
		{@render nestedRows(rows, childGroups, allowSelect, hl)}
	{/if}
{/snippet}

<!-- Shared section wrapper: card-detailed breaks out of the centered container
     to the full window width MINUS whatever the docked panels reserve on each
     edge (the layout's --dock-left-w / --dock-right-w); every other view stays
     centered. tsumikit's Container fullWidth bleeds to 100vw regardless, which
     slid the outer cards under an open panel. -->
{#snippet sectionsWrap(body: Snippet)}
	{#if cardView && !dense}
		<div class="bleed">
			<div class="sections">{@render body()}</div>
		</div>
	{:else}
		<div class="sections" class:tight={dense && !cardView}>{@render body()}</div>
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
				accentHue={accentOf(s)}
				pendingCount={pending(s.id)}
				unreadCount={openSession?.id === s.id ? 0 : (s.unread_count ?? 0)}
				onopen={(x) => (openSession = x)}
				selectable={allowSelect && list.selecting}
				selected={list.selected.has(s.id)}
				onToggleSelect={list.toggleSelect}
				swipeable
				swipeLabel={s.status === 'archived' ? m.sessions_unarchive() : m.sessions_archive()}
				onSwipe={swipeArchive}
				onTogglePin={depth > 0 ? undefined : togglePin}
				highlight={hl}
				subagentCost={costRollup(s, subGroups)}
				subagentToggles={collapsibleGroups.map((g) => ({
					key: g.key,
					count: g.agents.length,
					running: g.running,
					open: list.expanded.has(groupId(s.id, g.key)),
					label: g.label,
					ontoggle: () => list.toggleGroup(s.id, g.key)
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
				{#if g.agents.length < INLINE_THRESHOLD || list.expanded.has(groupId(s.id, g.key))}
					<div class="agent-children" style="--agent-depth: {Math.min(depth + 1, 5)}">
						{@render nestedRows(g.agents, childGroups, allowSelect, hl, depth + 1)}
					</div>
				{/if}
			{/each}
		{/if}
	{/each}
{/snippet}

{#snippet draftItems(rows: SessionListItem[], grid: boolean)}
	{#each rows as s (s.id)}
		<div class="parent-row" class:dense={dense && !grid}>
			<SessionCard
				session={s}
				{grid}
				compact={dense && !grid}
				accentHue={accentOf(s)}
				draft
				draftLaunching={launchingDraft === s.id}
				preview={draftPromptPreview(s)}
				onLaunch={launchDraft}
				onEdit={editDraft}
				onDiscard={discardDraft}
				onopen={() => {}}
			/>
		</div>
	{/each}
{/snippet}

{#snippet kanbanCard(s: SessionListItem)}
	{#if s.status === 'draft'}
		<SessionCard
			session={s}
			grid
			compact
			draft
			draftLaunching={launchingDraft === s.id}
			preview={draftPromptPreview(s)}
			onLaunch={launchDraft}
			onEdit={editDraft}
			onDiscard={discardDraft}
			onopen={() => {}}
		/>
	{:else}
		{@const subGroups = list.kanbanChildGroups.get(s.id) ?? []}
		<SessionCard
			session={s}
			grid
			compact
			accentHue={accentOf(s)}
			stacked={subGroups.length > 0}
			pendingCount={pending(s.id)}
			unreadCount={openSession?.id === s.id ? 0 : (s.unread_count ?? 0)}
			onopen={(x) => (openSession = x)}
			swipeable
			swipeLabel={m.sessions_archive()}
			onSwipe={swipeArchive}
			onTogglePin={togglePin}
			subagentCost={costRollup(s, subGroups)}
			subagentToggles={subGroups.map((g) => ({
				key: g.key,
				count: g.agents.length,
				running: g.running,
				open: false,
				label: g.label,
				ontoggle: () => {}
			}))}
			{allLabels}
			onCreateLabel={createLabel}
			onAttachLabel={attachLabel}
			onDetachLabel={detachLabel}
			onUpdateLabel={updateLabel}
			onDeleteLabel={deleteLabel}
		/>
	{/if}
{/snippet}

{#snippet loadMore()}
	{#if pageError}
		<div class="empty err"><Text tone="danger">{m.sessions_search_failed({ error: pageError })}</Text></div>
	{:else if pageLoading}
		<div class="loadmore"><span class="spin"></span></div>
		{:else if !pageDone && pageRows.length > 0}
			<div class="loadmore">
				<Button onclick={() => loadPage(false)}>{m.sessions_load_more()}</Button>
			</div>
		{/if}
{/snippet}

<!-- The height floor keeps the document from collapsing under the sticky
     toolbar while a search swaps the tall live list for a short results block —
     without it the window scrollTop clamps and the bar jumps mid-type. -->
<div class="list-area">
	{#if searching}
		<!-- Search results, scoped by the Archived checkbox; split Live / Archived. -->
		{#if pageLoading && pageRows.length === 0}
			<div class="empty"><span class="spin"></span></div>
		{:else if pageRows.length === 0}
			<div class="empty"><Text tone="muted">{m.sessions_search_no_match({ query: serverQuery })}{showArchived ? '.' : ' ' + m.sessions_search_live_only_hint()}</Text></div>
		{:else}
			{@render sectionsWrap(searchSections)}
		{/if}
	{:else if kanban}
		{#if sessions.isLoading}
			<div class="empty"><span class="spin"></span></div>
		{:else}
			<div class="bleed">
				<KanbanBoard columns={list.kanbanColumns} card={kanbanCard} />
			</div>
		{/if}
	{:else}
		{@render sectionsWrap(liveSections)}
	{/if}
</div>

{#snippet searchSections()}
	<!-- Nest over the whole result set so a parent and its subagents stay
	     grouped even if they land in different status sections; then split
	     the top-level rows into Live / Archived. -->
	{@const ns = nest(pageRows)}
	{@const scoped = ns.topLevel.filter(keepRow)}
	{@const liveTop = scoped.filter((s) => s.status !== 'archived')}
	{@const archTop = scoped.filter((s) => s.status === 'archived')}
	{#if liveTop.length > 0}
		<div class="section">
			<div class="group-header">{m.sessions_section_live()} <Text class="count">{liveTop.length}</Text></div>
			{@render rowsView(liveTop, ns.childGroups, false, searchTerms)}
		</div>
	{/if}
	{#if archTop.length > 0}
		<div class="section">
			<div class="group-header">
				{m.sessions_section_archived()} <Text class="count">{archTop.length}</Text>
				{#if showArchived}
					<IconButton
						inline
						icon="eye-off"
						size={14}
						label={m.sessions_hide_archived()}
						title={m.sessions_hide_archived()}
						onclick={hideArchived}
					/>
				{/if}
			</div>
			{@render rowsView(archTop, ns.childGroups, false, searchTerms)}
		</div>
	{/if}
	{#if scoped.length === 0}
		<div class="empty"><Text tone="muted">{m.sessions_search_no_sections()}</Text></div>
	{/if}
	{@render loadMore()}
{/snippet}

{#snippet dimHeader(label: string, count: number, hue: number | null)}
	<div class="group-header dim-header">
		{#if hue !== null}<span class="dim-swatch" style="--mh:{hue}"></span>{/if}
		{label} <Text class="count">{count}</Text>
	</div>
{/snippet}

{#snippet liveSections()}
		{#if sessions.isLoading}
			<div class="empty"><span class="spin"></span></div>
		{:else if !list.hasLiveRows && !showArchived && !(sections.has('drafts') && list.draftRows.length > 0)}
			<div class="empty">
				<Text tone="muted">{m.sessions_empty_sections()}</Text>
			</div>
		{:else if groupBy !== 'none'}
			{#each list.groupedSections as g (g.key)}
				<div class="section">
					{@render dimHeader(g.label, g.sessions.length, g.hue)}
					{@render rowsView(g.sessions, childGroupsOf, true, [])}
				</div>
			{/each}
		{:else}
			{#each list.groups as g (g.key)}
				{@const vis = g.sessions}
				<div class="section">
					{#if g.key === 'dispatched'}
						<!-- Dispatched is a plain section header like Pinned/Completed, with a
						     bulk "Archive all" action on the right. -->
						<div class="group-header" data-bucket={g.key}>
							{g.label} <Text class="count">{g.sessions.length}</Text>
							<!-- In card mode the action sits right next to the title; in
							     list mode it's pushed to the far right via the spacer. -->
							{#if !cardView}<div class="spacer"></div>{/if}
							<Button
								variant="danger"
								disabled={archiving}
								title={m.sessions_archive_all_dispatched_title()}
								onclick={archiveAllDispatched}
							>
								{#if archiving}<span class="spin"></span>{/if}
								{m.sessions_archive_all()}
							</Button>
						</div>
					{:else}
						<div class="group-header" data-bucket={g.key}>
							{g.label} <Text class="count">{g.sessions.length}</Text>
						</div>
					{/if}
					{@render rowsView(vis, childGroupsOf, true, [])}
				</div>
			{/each}
		{/if}

		{#if sections.has('drafts') && list.draftRows.length > 0}
			<!-- Drafts render through the SAME SessionCard path as every other
			     section, so they honor the card-view / compact toggles
			     identically; the card surfaces Launch/Edit/Discard in place of the
			     live-session affordances. -->
			<div class="section">
				<div class="group-header">{m.sessions_section_drafts()} <Text class="count">{list.draftRows.length}</Text></div>
				{#if cardView}
					{#if dense}
						<AutoGrid min="calc(18rem * var(--fs-scale))" max="calc(26.75rem * var(--fs-scale))" maxCols={2} gap="var(--sp-2)">{@render draftItems(list.draftRows, true)}</AutoGrid>
					{:else}
						<AutoGrid min="calc(20rem * var(--fs-scale))" max="calc(26.75rem * var(--fs-scale))" gap="var(--sp-3)">{@render draftItems(list.draftRows, true)}</AutoGrid>
					{/if}
				{:else}
					{@render draftItems(list.draftRows, false)}
				{/if}
			</div>
		{/if}

		{#if showArchived}
			{@const ns = nest(pageRows)}
			{@const archTop = ns.topLevel.filter(
				(s) => keepRow(s) && !pinnedArchivedKidIds.has(s.id)
			)}
			<div class="section">
				<div class="group-header">
				{m.sessions_section_archived()} <Text class="count">{archTop.length}</Text>
				{#if showArchived}
					<IconButton
						inline
						icon="eye-off"
						size={14}
						label={m.sessions_hide_archived()}
						title={m.sessions_hide_archived()}
						onclick={hideArchived}
					/>
				{/if}
			</div>
				{#if pageRows.length === 0 && !pageLoading}
					<div class="empty"><Text tone="muted">{m.sessions_no_archived()}</Text></div>
				{:else}
					{@render rowsView(archTop, ns.childGroups, false, searchTerms)}
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

{#if dockSide}
	{#key dockEpoch}
		<SpawnModal
			docked={dockSide}
			stacked={docks.stacked}
			dockWidth={docks[dockSide] ?? undefined}
			prefill={spawnPrefill}
			onclose={() => {
				spawnPrefill = null;
				dockEpoch++;
			}}
			onspawned={() => qc.invalidateQueries({ queryKey: ['sessions'] })}
		/>
	{/key}
{:else if showSpawn}
	<SpawnModal
		prefill={spawnPrefill}
		onclose={() => {
			showSpawn = false;
			spawnPrefill = null;
		}}
		onspawned={() => qc.invalidateQueries({ queryKey: ['sessions'] })}
	/>
{/if}

{#if docks.stats && docks[docks.stats]}
	<StatsDock side={docks.stats} stacked={docks.stacked} width={docks[docks.stats] ?? ''} />
{/if}

<style>
	/* Sticky bulk-action bar shown while in select mode. */
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
	/* Card-detailed breakout: the same "pull each edge out to the viewport"
	   trick as tsumikit's Container fullWidth, but the span stops at the docked
	   panels. The centered parent sits in the middle of the free strip, so the
	   negative margin is half the difference between the parent and the strip. */
	.bleed {
		--bleed-w: calc(100vw - var(--dock-left-w, 0px) - var(--dock-right-w, 0px));
		width: var(--bleed-w);
		max-width: none;
		margin-inline: calc(50% - var(--bleed-w) / 2);
		padding-left: max(var(--sp-4), var(--safe-left));
		padding-right: max(var(--sp-4), var(--safe-right));
	}
	.section {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.sections.tight .section {
		gap: var(--sp-1);
	}
	/* Parent row: a normal full-width row. The collapse toggle badge(s)
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
	/* Group-by header: a hue swatch keyed to the color-by palette, in the
	   dimension's own casing (label/dir/machine names aren't uppercased chrome). */
	.dim-header {
		text-transform: none;
		letter-spacing: normal;
	}
	.dim-swatch {
		flex: none;
		width: 0.7rem;
		height: 0.7rem;
		border-radius: var(--r-sm);
		background: hsl(var(--mh) var(--mach-border-sl));
	}
	/* Dispatched group collapse toggle. */
	.group-header[data-bucket='blocked'] {
		color: var(--warn);
	}
	.group-header[data-bucket='review'] {
		color: var(--accent);
	}
</style>
