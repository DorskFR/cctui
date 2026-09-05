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
	import { Button, Callout, Dot, IconButton, Menu, Modal, SectionHeader, Text } from '@dorsk/tsumikit';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import { useAllMachines } from '$lib/queries';
	import {
		drafts,
		clearSpawnSlot,
		currentSpawnSlot,
		readSpawnSlot,
		LIST_VIEW,
		LIST_SECTION,
		LIST_LABELS,
		LIST_HIDDEN
	} from '$lib/drafts';
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
		parseHiddenSections,
		serializeHiddenSections,
		toggleHiddenSection,
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
		draftPreview,
		editDraftNeedsConfirm,
		spawnRequestFromSlot,
		type Section,
		type SubGroup,
		type Dimension,
		toGroupDimension
	} from './sessions.logic';
	import { SessionsListController } from './SessionsListController.svelte';

	// Two layouts: list (compact rows, centered column) and card (detailed 3-up
	// grid released to the full window). Grid is top-level only (subagents stay
	// in list view / the drawer).
	let cardView = $state(drafts.get(LIST_VIEW) === 'card');
	$effect(() => {
		drafts.set(LIST_VIEW, cardView ? 'card' : 'list');
	});


	// Color-by and group-by dimensions, read live from the
	// server-persisted settings blob (so an async settings.load() reflows the UI)
	// and written back through settings.setSessionList (localStorage + debounced PUT).
	const colorBy = $derived(settings.state.sessionList.colorBy as Dimension);
	const groupBy = $derived(settings.state.sessionList.groupBy as Dimension);
	const accentOf = (s: SessionListItem) => colorHueOf(s, colorBy);
	const showMachine = $derived(groupBy !== 'machine');

	// Machine liveness for the group headers; falls back to "unknown" (no dot)
	// when the machines endpoint is not readable.
	const machines = useAllMachines(() => groupBy === 'machine');
	const machineLiveness = (name: string): 'online' | 'stale' | 'offline' | null =>
		(machines.data ?? []).find((mc) => mc.name === name)?.liveness ?? null;
	const sortItems = $derived(
		(['activity', 'created', 'name'] as const).map((sort) => ({
			label: sortLabel(sort),
			onselect: () => settings.setSessionList({ sort })
		}))
	);
	function sortLabel(sort: string): string {
		return sort === 'created'
			? m.settings_sort_created()
			: sort === 'name'
				? m.settings_sort_name()
				: m.settings_sort_activity();
	}
	function bucketColor(key: string | null | undefined): string {
		switch (key) {
			case 'blocked':
				return 'var(--warn)';
			case 'review':
				return 'var(--accent)';
			case 'working':
				return 'var(--ok)';
			case 'dispatched':
				return 'var(--info)';
			default:
				return 'var(--text-faint)';
		}
	}

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
	// Per-section collapse, independent of the section filter: every group
	// header carries an eye toggle that drops its rows while keeping the header
	// and its live count. Persisted alongside the section set.
	let hiddenSections = $state<Set<string>>(parseHiddenSections(drafts.get(LIST_HIDDEN)));
	$effect(() => {
		drafts.set(LIST_HIDDEN, serializeHiddenSections(hiddenSections));
	});
	const toggleSection = (key: string) => {
		hiddenSections = toggleHiddenSection(hiddenSections, key);
	};
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
	let spawnModal = $state<ReturnType<typeof SpawnModal> | null>(null);
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
				toasts.error(m.sessions_toast_not_found());
				openSession = null;
				setUrlSession(null, true);
			} else {
				toasts.error(m.sessions_toast_open_failed({ error: errMessage(e) }));
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
		toasts.error(m.sessions_toast_fork_slow());
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
			toasts.ok(m.sessions_toast_archived({ count: ids.length }), undefined, undoArchive(ids));
			list.exitSelect();
			refreshTick++;
		} catch (e) {
			toasts.error(errMessage(e));
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
			toasts.ok(m.sessions_toast_archived({ count: ids.length }), undefined, undoArchive(ids));
			refreshTick++;
			qc.invalidateQueries({ queryKey: ['sessions'] });
		} catch (e) {
			toasts.error(errMessage(e));
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
				undefined,
				isArchived ? undefined : undoArchive([s.id])
			);
			refreshTick++;
		} catch (e) {
			toasts.error(errMessage(e));
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
			toasts.error(errMessage(e));
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
			clearSpawnSlot(s.machine_id, s.working_dir);
			toasts.ok(m.sessions_toast_draft_launched());
		} catch (e) {
			toasts.error(m.sessions_toast_launch_failed({ error: errMessage(e) }));
		} finally {
			launchingDraft = null;
		}
	}

	async function discardDraft(s: SessionListItem) {
		if (!confirm(m.sessions_confirm_discard_draft())) return;
		try {
			await actions.discardDraft(s.id);
			clearSpawnSlot(s.machine_id, s.working_dir);
			toasts.ok(m.sessions_toast_draft_discarded());
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

	// Edit a draft: open the spawn form on the draft's row (updated in place,
	// deleted only on launch). A form already holding someone else's content
	// asks first — replace it, or save it as its own draft before.
	let pendingDraftEdit = $state<SessionListItem | null>(null);
	function editDraft(s: SessionListItem) {
		const live = spawnModal
			? { dirty: spawnModal.isDirty(), draftId: spawnModal.currentDraftId() }
			: null;
		if (editDraftNeedsConfirm(s.id, live, readSpawnSlot(currentSpawnSlot()))) {
			pendingDraftEdit = s;
			return;
		}
		openSpawn(draftEditPrefill(s));
	}
	async function saveCurrentSpawnForm() {
		if (spawnModal) {
			if (!(await spawnModal.flushDraft())) throw new Error(m.spawn_draft_incomplete());
			return;
		}
		const key = currentSpawnSlot();
		const slot = readSpawnSlot(key);
		const body = slot && spawnRequestFromSlot(slot);
		if (!body) throw new Error(m.spawn_draft_incomplete());
		if (slot.draftId) {
			await actions.updateDraft(slot.draftId, body);
			return;
		}
		const res = await actions.spawn({ ...body, save_draft: true }, []);
		drafts.set(key, JSON.stringify({ ...slot, draftId: String(res.command_id) }));
	}
	async function confirmDraftEdit(saveFirst: boolean) {
		const s = pendingDraftEdit;
		pendingDraftEdit = null;
		if (!s) return;
		if (saveFirst) {
			try {
				await saveCurrentSpawnForm();
				toasts.ok(m.sessions_toast_current_saved_draft());
			} catch (e) {
				toasts.error(m.sessions_toast_save_current_failed({ error: errMessage(e) }));
				return;
			}
		}
		openSpawn(draftEditPrefill(s));
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

	// The list derivations (nest/buckets/group-by) + multi-select and
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
	{colorBy}
	{groupBy}
	onColorBy={(v) => settings.setSessionList({ colorBy: v === 'status' ? 'none' : v })}
	onGroupBy={(v) => settings.setSessionList({ groupBy: toGroupDimension(v) })}
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
			{showMachine}
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
	<div class="card-grid">{@render cardItems(rows, childGroups, hl)}</div>
{/snippet}

<!-- Every row set — live buckets, search results, archive browse — renders
     through this one card-vs-list dispatch so no branch can drift from the
     view picker. -->
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
	{#if cardView}
		<div class="bleed">
			<div class="sections">{@render body()}</div>
		</div>
	{:else}
		<div class="sections tight">{@render body()}</div>
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
		<div class="parent-row">
			<SessionCard
				session={s}
				variant="row"
				child={depth > 0}
				{showMachine}
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
		<div class="parent-row">
			<SessionCard
				session={s}
				variant={grid ? 'card' : 'row'}
				{showMachine}
				accentHue={accentOf(s)}
				draft
				draftLaunching={launchingDraft === s.id}
				preview={draftPreview(s)}
				onLaunch={launchDraft}
				onEdit={editDraft}
				onDiscard={discardDraft}
				onopen={() => {}}
			/>
		</div>
	{/each}
{/snippet}

{#snippet loadMore()}
	{#if pageError}
		<Callout tone="danger">{m.sessions_search_failed({ error: pageError })}</Callout>
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
			<div class="placeholder"><span class="spin"></span></div>
		{:else if pageRows.length === 0}
			<div class="placeholder"><Text tone="muted">{m.sessions_search_no_match({ query: serverQuery })}{showArchived ? '.' : ' ' + m.sessions_search_live_only_hint()}</Text></div>
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
	{@const ns = nest(pageRows)}
	{@const scoped = ns.topLevel.filter(keepRow)}
	{@const liveTop = scoped.filter((s) => s.status !== 'archived')}
	{@const archTop = scoped.filter((s) => s.status === 'archived')}
	<div class="section">
		{@render groupHeader('live', m.sessions_section_live(), liveTop.length, {})}
		{#if !hiddenSections.has('live')}
			{@render rowsView(liveTop, ns.childGroups, false, searchTerms)}
		{/if}
	</div>
	{#if showArchived}
		<div class="section">
			{@render groupHeader('archived', m.sessions_section_archived(), archTop.length, {})}
			{#if !hiddenSections.has('archived')}
				{@render rowsView(archTop, ns.childGroups, false, searchTerms)}
			{/if}
		</div>
	{/if}
	{#if scoped.length === 0}
		<div class="placeholder"><Text tone="muted">{m.sessions_search_no_sections()}</Text></div>
	{/if}
	{@render loadMore()}
{/snippet}

<!-- One header for every section: label, live count, and an eye toggle that
     collapses the section's rows while leaving the header in place. -->
{#snippet groupHeader(
	key: string,
	label: string,
	count: number,
	opts: { hue?: number | null; bucket?: string | null; machine?: string | null; trailing?: Snippet }
)}
	{@const hidden = hiddenSections.has(key)}
	{@const action = hidden
		? m.sessions_section_show({ section: label })
		: m.sessions_section_hide({ section: label })}
	{@const liveness = opts.machine ? machineLiveness(opts.machine) : null}
	<SectionHeader
		variant="group"
		level={3}
		size="sm"
		title={opts.machine ? '' : label}
		hue={opts.machine || opts.hue == null ? undefined : opts.hue}
		count={m.sessions_group_count({ count })}
		lead={headerLead}
		actions={headerActions}
	/>
	{#snippet headerLead()}
		{#if opts.machine}
			<MachineBadge name={opts.machine} id={opts.machine} hue={opts.hue} mono />
			{#if liveness}
				<span class="liveness" class:online={liveness === 'online'}>
					<Dot status={liveness === 'online' ? 'active' : liveness === 'stale' ? 'stale' : 'dead'} />
					{liveness === 'online'
						? m.sessions_machine_online()
						: liveness === 'stale'
							? m.sessions_machine_stale()
							: m.sessions_machine_offline()}
				</span>
			{/if}
		{:else if opts.bucket}
			<Dot color={bucketColor(opts.bucket)} />
		{/if}
	{/snippet}
	{#snippet headerActions()}
		<Menu label={m.sessions_sort_menu_label()} items={sortItems} bare placement="bottom-end">
			{#snippet trigger()}
				<Text size="xs" tone="faint" style="white-space:nowrap"
					>{m.sessions_sort_menu({ sort: sortLabel(settings.state.sessionList.sort) })} ▾</Text
				>
			{/snippet}
		</Menu>
		<IconButton
			inline
			icon={hidden ? 'eye' : 'eye-off'}
			size={14}
			label={action}
			title={action}
			onclick={() => toggleSection(key)}
		/>
		{#if opts.trailing}{@render opts.trailing()}{/if}
	{/snippet}
{/snippet}

{#snippet archiveAllDispatchedAction()}
	<Button
		variant="danger"
		disabled={archiving}
		title={m.sessions_archive_all_dispatched_title()}
		onclick={archiveAllDispatched}
	>
		{#if archiving}<span class="spin"></span>{/if}
		{m.sessions_archive_all()}
	</Button>
{/snippet}

{#snippet liveSections()}
		{#if sessions.isLoading}
			<div class="placeholder"><span class="spin"></span></div>
		{:else if !list.hasLiveRows && !showArchived && !sections.has('drafts')}
			<div class="placeholder">
				<Text tone="muted">{m.sessions_empty_sections()}</Text>
			</div>
		{:else if groupBy !== 'status'}
			{#each list.groupedSections as g (g.key)}
				{@const key = `dim:${g.key}`}
				<div class="section">
					{@render groupHeader(key, g.label, g.sessions.length, {
						hue: g.hue,
						machine: groupBy === 'machine' && g.hue !== null ? g.label : null
					})}
					{#if !hiddenSections.has(key)}
						{@render rowsView(g.sessions, childGroupsOf, true, [])}
					{/if}
				</div>
			{/each}
		{:else}
			{#each list.groups as g (g.key)}
				<div class="section">
					{@render groupHeader(g.key, g.label, g.sessions.length, {
						bucket: g.key,
						trailing: g.key === 'dispatched' ? archiveAllDispatchedAction : undefined
					})}
					{#if !hiddenSections.has(g.key)}
						{@render rowsView(g.sessions, childGroupsOf, true, [])}
					{/if}
				</div>
			{/each}
		{/if}

		{#if sections.has('drafts')}
			<!-- Drafts render through the SAME SessionCard path as every other
			     section, so they honor the card-view / compact toggles
			     identically; the card surfaces Launch/Edit/Discard in place of the
			     live-session affordances. -->
			<div class="section">
				{@render groupHeader('drafts', m.sessions_section_drafts(), list.draftRows.length, {})}
				{#if !hiddenSections.has('drafts')}
					{#if cardView}
						<div class="card-grid">{@render draftItems(list.draftRows, true)}</div>
					{:else}
						{@render draftItems(list.draftRows, false)}
					{/if}
				{/if}
			</div>
		{/if}

		{#if showArchived}
			{@const ns = nest(pageRows)}
			{@const archTop = ns.topLevel.filter(
				(s) => keepRow(s) && !pinnedArchivedKidIds.has(s.id)
			)}
			<div class="section">
				{@render groupHeader('archived', m.sessions_section_archived(), archTop.length, {})}
				{#if !hiddenSections.has('archived')}
					{#if pageRows.length === 0 && !pageLoading}
						<div class="placeholder"><Text tone="muted">{m.sessions_no_archived()}</Text></div>
					{:else}
						{@render rowsView(archTop, ns.childGroups, false, searchTerms)}
						{@render loadMore()}
					{/if}
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
			bind:this={spawnModal}
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
		bind:this={spawnModal}
		prefill={spawnPrefill}
		onclose={() => {
			showSpawn = false;
			spawnPrefill = null;
		}}
		onspawned={() => qc.invalidateQueries({ queryKey: ['sessions'] })}
	/>
{/if}

{#if pendingDraftEdit}
	<Modal title={m.sessions_edit_draft_title()} onclose={() => (pendingDraftEdit = null)} size="sm">
		{#snippet body()}
			<Text>{m.sessions_edit_draft_body()}</Text>
		{/snippet}
		{#snippet footer()}
			<Button block onclick={() => (pendingDraftEdit = null)}>{m.common_cancel()}</Button>
			<Button block onclick={() => void confirmDraftEdit(true)}>{m.sessions_edit_draft_save_first()}</Button>
			<Button block variant="primary" onclick={() => void confirmDraftEdit(false)}>{m.sessions_edit_draft_replace()}</Button>
		{/snippet}
	</Modal>
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
	.liveness {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.liveness.online {
		color: var(--ok);
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
	.sections {
		container-type: inline-size;
	}
</style>
