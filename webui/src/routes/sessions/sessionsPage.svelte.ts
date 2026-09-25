import type { SessionListItem } from '@bindings/SessionListItem';
import type { Label } from '@bindings/Label';
import type { endpoints, useSessionActions } from '$lib/queries';
import type { toasts, ToastAction } from '$lib/toast.svelte';
import type { DockLayout } from '$lib/dock';
import type { SessionListSettings } from '$lib/settings.svelte';
import { LIST_HIDDEN, LIST_LABELS, LIST_SECTION, LIST_VIEW, type SpawnSlotPayload } from '$lib/drafts';
import { ApiError, errMessage } from '$lib/api';
import { m } from '$lib/paraglide/messages';
import { tokenizeQuery } from '$lib/search';
import { BRIEF_FETCH_MAX_BYTES, followupPrefill } from '$lib/followup';
import { freeText, parse, type Schema } from '@dorsk/tsumikit';
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
	nextSort,
	archivedDescendantsOf,
	inEnabledSections,
	matchesUnreadFilter,
	parseLabelFilter,
	colorHueOf,
	pickFreshSession,
	scriptPrefill,
	draftEditPrefill,
	editDraftNeedsConfirm,
	spawnRequestFromSlot,
	type Section,
	type SessionSort,
	type Dimension
} from './sessions.logic';
import { SessionsListController } from './SessionsListController.svelte';
import { ArchiveConfirm } from './archiveConfirm.svelte';

export type SessionActions = ReturnType<typeof useSessionActions>;

export interface SpawnForm {
	isDirty(): boolean;
	currentDraftId(): string | null;
	flushDraft(): Promise<boolean>;
}

export interface SessionsPageDeps {
	store: { get(key: string): string; set(key: string, value: string): void };
	settings: {
		state: { sessionList: SessionListSettings };
		setSessionList(patch: Partial<SessionListSettings>): void;
		readonly archiveDoneButton: boolean;
	};
	toasts: Pick<typeof toasts, 'ok' | 'error'>;
	api: Pick<typeof endpoints, 'session' | 'searchSessions' | 'brief' | 'searchFieldValues'>;
	actions: Pick<
		SessionActions,
		| 'archive'
		| 'unarchive'
		| 'archiveMany'
		| 'unarchiveMany'
		| 'pin'
		| 'unpin'
		| 'createLabel'
		| 'attachLabel'
		| 'detachLabel'
		| 'updateLabel'
		| 'deleteLabel'
		| 'launchDraft'
		| 'discardDraft'
		| 'updateDraft'
		| 'spawn'
	>;
	invalidateSessions: () => void;
	refetchLive: () => Promise<unknown>;
	dockLayout: () => DockLayout;
	clearUrlSession: () => void;
	confirm: (message: string) => boolean;
	spawnSlot: {
		currentKey: () => string;
		read: (key: string) => SpawnSlotPayload | null;
		write: (key: string, payload: SpawnSlotPayload) => void;
		clear: (machineId: string, workingDir: string) => void;
	};
	sessions: () => SessionListItem[];
	sessionsLoading: () => boolean;
	allSessions: () => SessionListItem[];
	labels: () => Label[] | undefined;
	// Visual document order of the rendered rows, for shift-range selection.
	renderedOrder: () => string[];
}

export class SessionsPage {
	#d: SessionsPageDeps;
	readonly list: SessionsListController;
	readonly archiveConfirm: ArchiveConfirm;
	readonly searchSchema: Schema;

	// Two layouts: list (compact rows, centered column) and card (detailed 3-up
	// grid released to the full window). Grid is top-level only (subagents stay
	// in list view / the drawer).
	cardView = $state(false);
	// Section filter: independent on/off toggles over the loaded list
	// (starred / live / dispatched / drafts / archived / unread), persisted.
	sections = $state<Set<Section>>(new Set());
	// Per-section collapse, independent of the section filter: the eye toggle
	// drops a group's rows while keeping the header and its live count.
	hiddenSections = $state<Set<string>>(new Set());
	// Selected label ids; when non-empty the live list and archive browse are
	// narrowed to sessions carrying at least one of them (OR semantics).
	labelFilter = $state(new Set<string>());

	openSession = $state<SessionListItem | null>(null);
	showSpawn = $state(false);
	// Bumped whenever the docked form is done with (spawned, drafted, cleared,
	// or handed a prefill) so it remounts and reseeds exactly like a reopened
	// modal would.
	dockEpoch = $state(0);
	spawnPrefill = $state<Record<string, string> | null>(null);
	spawnModal = $state<SpawnForm | null>(null);
	// Set while a URL-driven open is fetching, so the drawer→URL sync skips the echo.
	urlResolving = false;

	// `rawQuery` is the live input, debounced into `query`.
	rawQuery = $state('');
	query = $state('');

	pageRows = $state<SessionListItem[]>([]);
	pageOffset = $state(0);
	pageDone = $state(false);
	pageLoading = $state(false);
	pageError = $state('');
	#pageReqId = 0;
	// Bump to reload the pager after archive ops.
	refreshTick = $state(0);

	archivingOne = $state(false);
	launchingDraft = $state<string | null>(null);
	pendingDraftEdit = $state<SessionListItem | null>(null);
	// Opening a CARD while searching focuses that card's transcript hit. Keyed
	// by session id so every other open path and every list refetch leave it null.
	focusHit = $state<{ id: string; seq: number } | null>(null);

	constructor(d: SessionsPageDeps) {
		this.#d = d;
		this.cardView = d.store.get(LIST_VIEW) === 'card';
		this.sections = parseSections(d.store.get(LIST_SECTION));
		this.hiddenSections = parseHiddenSections(d.store.get(LIST_HIDDEN));
		this.labelFilter = new Set<string>(parseLabelFilter(d.store.get(LIST_LABELS)));
		this.searchSchema = buildSessionSearchSchema((field, q) =>
			d.api.searchFieldValues(field, q, contextForField(this.rawQuery, this.searchSchema, field))
		);
		this.archiveConfirm = new ArchiveConfirm(this.runArchive, (e) => d.toasts.error(errMessage(e)));
		this.list = new SessionsListController({
			items: () => this.items,
			pinnedArchivedKids: () => this.pinnedArchivedKids,
			sections: () => this.sections,
			groupBy: () => this.groupBy,
			sort: () => d.settings.state.sessionList.sort,
			sortDir: () => d.settings.state.sessionList.sortDir,
			matchesLabel: this.matchesLabelFilter,
			matchesClient: this.matchesClient,
			renderedOrder: d.renderedOrder
		});

		$effect(() => {
			d.store.set(LIST_VIEW, this.cardView ? 'card' : 'list');
		});
		$effect(() => {
			d.store.set(LIST_SECTION, [...this.sections].join(','));
		});
		$effect(() => {
			d.store.set(LIST_HIDDEN, serializeHiddenSections(this.hiddenSections));
		});
		$effect(() => {
			d.store.set(LIST_LABELS, [...this.labelFilter].join(','));
		});
		// Drop filter ids whose label was deleted so the count stays honest. Waits
		// for the label set to actually load: on a refresh the persisted filter is
		// restored while labels are still in flight, and pruning against an empty
		// set would wipe (and then persist) an empty filter.
		$effect(() => {
			if (!d.labels()) return;
			const known = new Set(this.allLabels.map((l) => l.id));
			if ([...this.labelFilter].some((id) => !known.has(id))) {
				this.labelFilter = new Set([...this.labelFilter].filter((id) => known.has(id)));
			}
		});
		$effect(() => {
			const v = this.rawQuery.trim();
			const t = setTimeout(() => (this.query = v), 200);
			return () => clearTimeout(t);
		});
		// Reset + reload page 0 whenever the mode (query / scope) changes, or an
		// archive action bumps refreshTick.
		$effect(() => {
			void this.pagerKey;
			void this.refreshTick;
			this.pageRows = [];
			this.pageOffset = 0;
			this.pageDone = false;
			this.pageError = '';
			if (this.pagerActive) this.loadPage(true);
		});
	}

	// Color-by and group-by dimensions, read live from the server-persisted
	// settings blob and written back through settings.setSessionList.
	colorBy = $derived(this.#d.settings.state.sessionList.colorBy as Dimension);
	groupBy = $derived(this.#d.settings.state.sessionList.groupBy as Dimension);
	showMachine = $derived(this.groupBy !== 'machine');
	sortState = $derived({
		sort: this.#d.settings.state.sessionList.sort,
		sortDir: this.#d.settings.state.sessionList.sortDir
	});
	// Drives the paginated archive pager + search scope.
	showArchived = $derived(this.sections.has('archived'));
	// Docked panels (Settings › New session / Stats panel): `spawn` null = modal mode.
	docks = $derived(this.#d.dockLayout());
	dockSide = $derived(this.docks.spawn);

	items = $derived(this.#d.sessions());
	sessionsLoading = $derived(this.#d.sessionsLoading());
	allLabels = $derived(this.#d.labels() ?? []);
	#loaded = $derived([...this.items, ...this.pageRows]);

	// Server-evaluable clauses + free text ride the raw string to the search
	// endpoint; `id`/`created` clauses are peeled off and narrowed client-side.
	#split = $derived(splitQuery(this.query, this.searchSchema));
	serverQuery = $derived(this.#split.serverQuery);
	#clientFilters = $derived(this.#split.clientFilters);
	searching = $derived(this.serverQuery.length > 0);
	// One pager feeds two views, never both at once: search results (scoped by
	// `showArchived`) or, with no query, the paged archive browse.
	pagerActive = $derived(this.searching || this.showArchived);
	// Highlight only the free-text portion of the query (field clauses excluded).
	searchTerms = $derived(tokenizeQuery(freeText(parse(this.query, this.searchSchema))));
	pagerKey = $derived(`${this.searching ? `q:${this.serverQuery}` : 'browse'}|${this.showArchived}`);

	archiving = $derived(this.archivingOne || this.archiveConfirm.busy);

	// A starred parent keeps its full subagent group visible under Pinned even
	// once the children are archived: the live list excludes archived rows, so
	// each pinned parent's archived descendants are spliced back into the nest
	// from the full list (fetched only while something is pinned).
	pinnedIds = $derived(new Set(this.items.filter((s) => s.pinned).map((s) => s.id)));
	#archivedPool = $derived(this.#d.allSessions().filter((s) => s.status === 'archived'));
	pinnedArchivedKids = $derived(archivedDescendantsOf(this.pinnedIds, this.#archivedPool));
	// Their ids, so the Archived browse doesn't also list them as their own
	// top-level rows.
	pinnedArchivedKidIds = $derived(new Set(this.pinnedArchivedKids.map((s) => s.id)));
	childGroupsOf = $derived(this.list.childGroupsOf);

	// The open drawer's session object, kept fresh as the lists refetch.
	liveOpen = $derived(pickFreshSession(this.openSession, this.#loaded));
	focusSeq = $derived(
		this.focusHit && this.focusHit.id === this.liveOpen?.id ? this.focusHit.seq : null
	);

	accentOf = (s: SessionListItem) => colorHueOf(s, this.colorBy);
	selectSort = (sort: SessionSort) => this.#d.settings.setSessionList(nextSort(this.sortState, sort));
	setColorBy = (v: Dimension) =>
		this.#d.settings.setSessionList({ colorBy: v === 'status' ? 'none' : v });
	toggleSection = (key: string) => {
		this.hiddenSections = toggleHiddenSection(this.hiddenSections, key);
	};
	get archiveDoneButton(): boolean {
		return this.#d.settings.archiveDoneButton;
	}

	matchesLabelFilter = (s: SessionListItem): boolean =>
		this.labelFilter.size === 0 || s.labels.some((l) => this.labelFilter.has(l.id));
	matchesClient = (s: SessionListItem): boolean => matchesClientFilters(s, this.#clientFilters);
	// One predicate for search results AND the archive browse so the two
	// branches can't drift apart on which filters apply.
	keepRow = (s: SessionListItem): boolean =>
		inEnabledSections(s, this.sections) &&
		this.matchesLabelFilter(s) &&
		this.matchesClient(s) &&
		matchesUnreadFilter(s, this.sections);

	createLabel = (name: string, color: string) => this.#d.actions.createLabel(name, color);
	attachLabel = (id: string, labelId: string) => this.#d.actions.attachLabel(id, labelId);
	detachLabel = (id: string, labelId: string) => this.#d.actions.detachLabel(id, labelId);
	updateLabel = (labelId: string, patch: { name?: string; color?: string }) =>
		this.#d.actions.updateLabel(labelId, patch);
	deleteLabel = (labelId: string) => this.#d.actions.deleteLabel(labelId);

	// Open the spawn form seeded with `prefill`: the modal, or a fresh mount of
	// the docked panel.
	openSpawn = (prefill: Record<string, string> | null) => {
		this.spawnPrefill = prefill;
		if (this.dockSide) this.dockEpoch++;
		else this.showSpawn = true;
	};
	newFromScript = (s: SessionListItem) => {
		this.openSession = null;
		this.openSpawn(scriptPrefill(s));
	};
	followUp = async (s: SessionListItem, instruction?: string) => {
		try {
			const brief = await this.#d.api.brief(s.id, BRIEF_FETCH_MAX_BYTES);
			this.openSession = null;
			this.openSpawn(followupPrefill(s, brief.markdown, { instruction }));
		} catch (e) {
			this.#d.toasts.error(m.followup_brief_failed({ error: errMessage(e) }));
		}
	};

	openFromCard = (s: SessionListItem) => {
		const seq = this.searchTerms.length ? s.match_seq : null;
		this.focusHit = seq == null ? null : { id: s.id, seq };
		this.openSession = s;
	};

	// Open a session by id, from the loaded lists or, if absent (purged from the
	// live view, on another page, etc.), fetched directly so a pasted link still
	// resolves.
	openById = async (id: string) => {
		const found = this.#loaded.find((s) => s.id === id);
		if (found) {
			this.openSession = found;
			return;
		}
		this.urlResolving = true;
		try {
			this.openSession = await this.#d.api.session(id);
		} catch (e) {
			// Archived/ended sessions resolve from the DB, so a 404 means the
			// session was actually DELETED — only then toast + drop the id.
			// Transient errors leave the URL intact so a retry/refresh can recover.
			if (e instanceof ApiError && e.status === 404) {
				this.#d.toasts.error(m.sessions_toast_not_found());
				this.openSession = null;
				this.#d.clearUrlSession();
			} else {
				this.#d.toasts.error(m.sessions_toast_open_failed({ error: errMessage(e) }));
			}
		} finally {
			this.urlResolving = false;
		}
	};

	// Open a freshly forked session: the server pre-minted its id but the DB row
	// only appears once the daemon launches the worker and the next roster poll
	// lands (~2-3s). Poll a few times so it opens in place without a manual
	// refresh, and without a false "not found" during the gap.
	navigateToForked = async (id: string) => {
		for (let i = 0; i < 16; i++) {
			const found = this.#loaded.find((s) => s.id === id);
			if (found) {
				this.openSession = found;
				return;
			}
			try {
				this.openSession = await this.#d.api.session(id);
				return;
			} catch {
				// not registered yet — keep polling
			}
			await this.#d.refetchLive();
			await new Promise((r) => setTimeout(r, 500));
		}
		this.#d.toasts.error(m.sessions_toast_fork_slow());
	};

	loadPage = async (reset: boolean) => {
		if (!this.pagerActive) return;
		const offset = reset ? 0 : this.pageOffset;
		const req = ++this.#pageReqId;
		this.pageLoading = true;
		this.pageError = '';
		try {
			// not searching ⇒ browse archive (empty q, archived scope).
			const res = await this.#d.api.searchSessions(
				this.serverQuery,
				this.searching ? this.showArchived : true,
				PAGE,
				offset
			);
			if (req !== this.#pageReqId) return;
			const rows = res.sessions;
			this.pageRows = reset ? rows : [...this.pageRows, ...rows];
			this.pageOffset = offset + rows.length;
			this.pageDone = rows.length < PAGE;
		} catch (e) {
			if (req === this.#pageReqId) this.pageError = errMessage(e);
		} finally {
			if (req === this.#pageReqId) this.pageLoading = false;
		}
	};

	// "Undo" action attached to every archive toast: un-archives the same ids
	// and refreshes the list. Only reachable while the toast is still visible.
	#undoArchive = (ids: string[]): ToastAction => ({
		label: m.toast_undo(),
		run: async () => {
			await this.#d.actions.unarchiveMany(ids);
			this.#d.toasts.ok(m.sessions_toast_unarchived());
			this.refreshTick++;
			this.#d.invalidateSessions();
		}
	});

	runArchive = async (ids: string[]) => {
		await this.#d.actions.archiveMany(ids);
		this.#d.toasts.ok(
			m.sessions_toast_archived({ count: ids.length }),
			undefined,
			this.#undoArchive(ids)
		);
		this.refreshTick++;
		this.#d.invalidateSessions();
	};

	archiveSelected = async () => {
		const ids = [...this.list.selected];
		if (ids.length === 0) return;
		if (ids.length > 1) {
			this.archiveConfirm.request({
				title: m.sessions_confirm_archive_many_title(),
				message: m.sessions_confirm_archive_many({ count: ids.length }),
				ids,
				onDone: this.list.exitSelect
			});
			return;
		}
		this.archivingOne = true;
		try {
			await this.runArchive(ids);
			this.list.exitSelect();
		} catch (e) {
			this.#d.toasts.error(errMessage(e));
		} finally {
			this.archivingOne = false;
		}
	};

	archiveSection = (label: string, ids: string[]) => {
		this.archiveConfirm.request({
			title: m.sessions_archive_section({ section: label }),
			message: m.sessions_confirm_archive_section({ count: ids.length, section: label }),
			ids
		});
	};

	// Swipe-to-archive a single row. Status-aware so it works for both live
	// (archive) and archived (unarchive) rows.
	swipeArchive = async (s: SessionListItem) => {
		const isArchived = s.status === 'archived';
		try {
			if (isArchived) await this.#d.actions.unarchive(s.id);
			else await this.#d.actions.archive(s.id);
			this.#d.toasts.ok(
				isArchived ? m.sessions_toast_unarchived() : m.sessions_toast_archived_one(),
				undefined,
				isArchived ? undefined : this.#undoArchive([s.id])
			);
			this.refreshTick++;
		} catch (e) {
			this.#d.toasts.error(errMessage(e));
		}
	};

	// Pinning floats a session to the top group and exempts it from auto-archive;
	// the list refetches so the move is immediate.
	togglePin = async (s: SessionListItem) => {
		try {
			if (s.pinned) await this.#d.actions.unpin(s.id);
			else await this.#d.actions.pin(s.id);
			this.#d.toasts.ok(s.pinned ? m.sessions_toast_unpinned() : m.sessions_toast_pinned());
			this.refreshTick++;
		} catch (e) {
			this.#d.toasts.error(errMessage(e));
		}
	};

	// Launch a draft: the server mints account env fresh at dispatch and removes
	// the draft row; the live session appears via the daemon's registration.
	launchDraft = async (s: SessionListItem) => {
		this.launchingDraft = s.id;
		try {
			await this.#d.actions.launchDraft(s.id);
			this.#d.spawnSlot.clear(s.machine_id, s.working_dir);
			this.#d.toasts.ok(m.sessions_toast_draft_launched());
		} catch (e) {
			this.#d.toasts.error(m.sessions_toast_launch_failed({ error: errMessage(e) }));
		} finally {
			this.launchingDraft = null;
		}
	};

	discardDraft = async (s: SessionListItem) => {
		if (!this.#d.confirm(m.sessions_confirm_discard_draft())) return;
		try {
			await this.#d.actions.discardDraft(s.id);
			this.#d.spawnSlot.clear(s.machine_id, s.working_dir);
			this.#d.toasts.ok(m.sessions_toast_draft_discarded());
		} catch (e) {
			this.#d.toasts.error(errMessage(e));
		}
	};

	// Edit a draft: open the spawn form on the draft's row (updated in place,
	// deleted only on launch). A form already holding someone else's content
	// asks first — replace it, or save it as its own draft before.
	editDraft = (s: SessionListItem) => {
		const form = this.spawnModal;
		const live = form ? { dirty: form.isDirty(), draftId: form.currentDraftId() } : null;
		if (editDraftNeedsConfirm(s.id, live, this.#d.spawnSlot.read(this.#d.spawnSlot.currentKey()))) {
			this.pendingDraftEdit = s;
			return;
		}
		this.openSpawn(draftEditPrefill(s));
	};
	#saveCurrentSpawnForm = async () => {
		const form = this.spawnModal;
		if (form) {
			if (!(await form.flushDraft())) throw new Error(m.spawn_draft_incomplete());
			return;
		}
		const key = this.#d.spawnSlot.currentKey();
		const slot = this.#d.spawnSlot.read(key);
		const body = slot && spawnRequestFromSlot(slot);
		if (!body) throw new Error(m.spawn_draft_incomplete());
		if (slot.draftId) {
			await this.#d.actions.updateDraft(slot.draftId, body);
			return;
		}
		const res = await this.#d.actions.spawn({ ...body, save_draft: true }, []);
		this.#d.spawnSlot.write(key, { ...slot, draftId: String(res.command_id) });
	};
	confirmDraftEdit = async (saveFirst: boolean) => {
		const s = this.pendingDraftEdit;
		this.pendingDraftEdit = null;
		if (!s) return;
		if (saveFirst) {
			try {
				await this.#saveCurrentSpawnForm();
				this.#d.toasts.ok(m.sessions_toast_current_saved_draft());
			} catch (e) {
				this.#d.toasts.error(m.sessions_toast_save_current_failed({ error: errMessage(e) }));
				return;
			}
		}
		this.openSpawn(draftEditPrefill(s));
	};
}
