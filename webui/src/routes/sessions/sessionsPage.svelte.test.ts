// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync } from 'svelte';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { Label } from '@bindings/Label';
import { ApiError } from '$lib/api';
import { LIST_HIDDEN, LIST_LABELS, LIST_SECTION, LIST_VIEW, type SpawnSlotPayload } from '$lib/drafts';
import type { SessionListSettings } from '$lib/settings.svelte';
import { SessionsPage, type SessionsPageDeps, type SpawnForm } from './sessionsPage.svelte';

function session(over: Partial<SessionListItem>): SessionListItem {
	return {
		id: 'sess',
		name: '',
		labels: [],
		working_dir: '/w',
		machine_id: 'm1',
		unread_count: 0,
		status: 'active',
		bucket: 'working',
		pinned: false,
		...over
	} as SessionListItem;
}

const label = (id: string): Label => ({ id, name: id, color: '#fff' }) as Label;

const defaultSettings = (): SessionListSettings =>
	({
		sort: 'activity',
		sortDir: 'desc',
		view: 'list',
		density: 'normal',
		section: '',
		labelFilter: [],
		colorBy: 'none',
		groupBy: 'status',
		width: 'default',
		accountNames: false
	}) as SessionListSettings;

function harness(over: Partial<SessionsPageDeps> = {}, stored: Record<string, string> = {}) {
	const store = new Map(Object.entries(stored));
	let items = $state.raw<SessionListItem[]>([]);
	let all = $state.raw<SessionListItem[]>([]);
	let labels = $state.raw<Label[] | undefined>(undefined);
	let slot: SpawnSlotPayload | null = null;
	const sessionList = $state(defaultSettings());
	const settings = {
		state: { sessionList },
		setSessionList(patch: Partial<SessionListSettings>) {
			Object.assign(sessionList, patch);
		},
		archiveDoneButton: false
	};
	const toasts = { ok: vi.fn(), error: vi.fn() };
	const api = {
		session: vi.fn(async (id: string) => session({ id })),
		searchSessions: vi.fn(async () => ({ sessions: [] as SessionListItem[] })),
		brief: vi.fn(async () => ({ markdown: '' })),
		searchFieldValues: vi.fn(async () => [] as string[])
	};
	const actions = {
		archive: vi.fn(async () => {}),
		unarchive: vi.fn(async () => {}),
		archiveMany: vi.fn(async () => {}),
		unarchiveMany: vi.fn(async () => {}),
		pin: vi.fn(async () => {}),
		unpin: vi.fn(async () => {}),
		createLabel: vi.fn(),
		attachLabel: vi.fn(),
		detachLabel: vi.fn(),
		updateLabel: vi.fn(),
		deleteLabel: vi.fn(),
		launchDraft: vi.fn(async () => {}),
		discardDraft: vi.fn(async () => {}),
		updateDraft: vi.fn(async () => {}),
		spawn: vi.fn(async () => ({ command_id: 7 }))
	};
	const deps: SessionsPageDeps = {
		store: { get: (k) => store.get(k) ?? '', set: (k, v) => void store.set(k, v) },
		settings,
		toasts,
		api: api as unknown as SessionsPageDeps['api'],
		actions: actions as unknown as SessionsPageDeps['actions'],
		invalidateSessions: vi.fn(),
		refetchLive: vi.fn(async () => {}),
		dockLayout: () => ({ spawn: null, stats: null, stacked: false, left: null, right: null }),
		clearUrlSession: vi.fn(),
		confirm: () => true,
		spawnSlot: {
			currentKey: () => 'slot',
			read: () => slot,
			write: vi.fn(),
			clear: vi.fn()
		},
		sessions: () => items,
		sessionsLoading: () => false,
		allSessions: () => all,
		labels: () => labels,
		renderedOrder: () => [],
		...over
	};
	let sp!: SessionsPage;
	const stop = $effect.root(() => {
		sp = new SessionsPage(deps);
	});
	flushSync();
	return {
		sp,
		stop,
		store,
		deps,
		api,
		actions,
		toasts,
		sessionList,
		setItems: (v: SessionListItem[]) => (items = v),
		setAll: (v: SessionListItem[]) => (all = v),
		setLabels: (v: Label[] | undefined) => (labels = v),
		setSlot: (v: SpawnSlotPayload | null) => (slot = v)
	};
}

let cleanup: (() => void)[] = [];
const make = (...args: Parameters<typeof harness>) => {
	const h = harness(...args);
	cleanup.push(h.stop);
	return h;
};

beforeEach(() => {
	vi.useFakeTimers();
});
afterEach(() => {
	for (const stop of cleanup) stop();
	cleanup = [];
	vi.useRealTimers();
});

describe('SessionsPage — persisted preferences', () => {
	it('restores view, sections, hidden sections and label filter from the store', () => {
		const { sp } = make(
			{},
			{
				[LIST_VIEW]: 'card',
				[LIST_SECTION]: 'live,archived',
				[LIST_HIDDEN]: 'done,drafts',
				[LIST_LABELS]: 'a,b'
			}
		);
		expect(sp.cardView).toBe(true);
		expect([...sp.sections]).toEqual(['live', 'archived']);
		expect(sp.showArchived).toBe(true);
		expect([...sp.hiddenSections]).toEqual(['done', 'drafts']);
		expect([...sp.labelFilter]).toEqual(['a', 'b']);
	});

	it('falls back to the default sections when nothing is stored', () => {
		const { sp } = make();
		expect([...sp.sections]).toEqual(['starred', 'live', 'dispatched']);
		expect(sp.cardView).toBe(false);
	});

	it('writes every preference back as it changes', () => {
		const { sp, store } = make();
		sp.cardView = true;
		sp.sections = new Set(['archived']);
		sp.toggleSection('working');
		sp.labelFilter = new Set(['x']);
		flushSync();
		expect(store.get(LIST_VIEW)).toBe('card');
		expect(store.get(LIST_SECTION)).toBe('archived');
		expect(store.get(LIST_HIDDEN)).toBe('working');
		expect(store.get(LIST_LABELS)).toBe('x');
		sp.toggleSection('working');
		flushSync();
		expect(store.get(LIST_HIDDEN)).toBe('');
	});

	it('prunes deleted label ids only once the label set has loaded', () => {
		const { sp, setLabels } = make({}, { [LIST_LABELS]: 'keep,gone' });
		flushSync();
		expect([...sp.labelFilter]).toEqual(['keep', 'gone']);
		setLabels([label('keep')]);
		flushSync();
		expect([...sp.labelFilter]).toEqual(['keep']);
	});
});

describe('SessionsPage — filtering', () => {
	it('matches on any selected label (OR) and passes everything with no filter', () => {
		const { sp } = make();
		const a = session({ id: 'a', labels: [label('l1')] });
		const b = session({ id: 'b', labels: [label('l2')] });
		expect(sp.matchesLabelFilter(a)).toBe(true);
		sp.labelFilter = new Set(['l1', 'l3']);
		expect(sp.matchesLabelFilter(a)).toBe(true);
		expect(sp.matchesLabelFilter(b)).toBe(false);
	});

	it('keepRow honours the enabled sections, the unread toggle and the label filter', () => {
		const { sp } = make();
		const live = session({ id: 'live', unread_count: 0 });
		const archived = session({ id: 'arch', status: 'archived' });
		expect(sp.keepRow(live)).toBe(true);
		expect(sp.keepRow(archived)).toBe(false);
		sp.sections = new Set(['live', 'archived', 'unread']);
		expect(sp.keepRow(archived)).toBe(false);
		expect(sp.keepRow(session({ id: 'arch2', status: 'archived', unread_count: 2 }))).toBe(true);
		sp.sections = new Set(['live']);
		sp.labelFilter = new Set(['l1']);
		expect(sp.keepRow(live)).toBe(false);
	});

	it('narrows the list by client-side clauses once the query settles', async () => {
		const { sp } = make();
		sp.rawQuery = 'id:abc';
		expect(sp.query).toBe('');
		await vi.advanceTimersByTimeAsync(200);
		flushSync();
		expect(sp.query).toBe('id:abc');
		expect(sp.matchesClient(session({ id: 'abcdef' }))).toBe(true);
		expect(sp.matchesClient(session({ id: 'zzz' }))).toBe(false);
	});

	it('feeds the label and client predicates into the list controller buckets', () => {
		const { sp, setItems } = make();
		setItems([
			session({ id: 'a', labels: [label('l1')] }),
			session({ id: 'b', labels: [] })
		]);
		sp.labelFilter = new Set(['l1']);
		const working = sp.list.groups.find((g) => g.key === 'working')!;
		expect(working.sessions.map((s) => s.id)).toEqual(['a']);
	});
});

describe('SessionsPage — sort', () => {
	it('reads the sort state from settings and orders the buckets with it', () => {
		const { sp, setItems, sessionList } = make();
		setItems([session({ id: 'b', name: 'beta' }), session({ id: 'a', name: 'alpha' })]);
		expect(sp.sortState).toEqual({ sort: 'activity', sortDir: 'desc' });
		expect(sp.list.groups.find((g) => g.key === 'working')!.sessions.map((s) => s.id)).toEqual([
			'b',
			'a'
		]);
		sessionList.sort = 'name';
		sessionList.sortDir = 'asc';
		expect(sp.list.groups.find((g) => g.key === 'working')!.sessions.map((s) => s.id)).toEqual([
			'a',
			'b'
		]);
	});

	it('selectSort picks a new field at its natural direction and flips the active one', () => {
		const { sp, sessionList } = make();
		sp.selectSort('name');
		expect(sessionList).toMatchObject({ sort: 'name', sortDir: 'asc' });
		sp.selectSort('name');
		expect(sessionList).toMatchObject({ sort: 'name', sortDir: 'desc' });
		sp.selectSort('created');
		expect(sessionList).toMatchObject({ sort: 'created', sortDir: 'desc' });
	});

	it('maps the color-by "status" choice to none', () => {
		const { sp, sessionList } = make();
		sp.setColorBy('status');
		expect(sessionList.colorBy).toBe('none');
		sp.setColorBy('label');
		expect(sessionList.colorBy).toBe('label');
	});
});

describe('SessionsPage — search and archive pager', () => {
	it('stays idle with no query and no archived section', () => {
		const { sp, api } = make();
		expect(sp.pagerActive).toBe(false);
		expect(api.searchSessions).not.toHaveBeenCalled();
	});

	it('browses the archive when the archived section is enabled', async () => {
		const { sp, api } = make();
		api.searchSessions.mockResolvedValueOnce({
			sessions: [session({ id: 'old', status: 'archived' })]
		});
		sp.sections = new Set(['live', 'archived']);
		flushSync();
		await vi.advanceTimersByTimeAsync(0);
		expect(api.searchSessions).toHaveBeenCalledWith('', true, 50, 0);
		expect(sp.pageRows.map((s) => s.id)).toEqual(['old']);
		expect(sp.pageDone).toBe(true);
		expect(sp.pageLoading).toBe(false);
	});

	it('searches with the live-only scope and reloads page 0 when the query changes', async () => {
		const { sp, api } = make();
		sp.rawQuery = 'needle';
		await vi.advanceTimersByTimeAsync(200);
		flushSync();
		await vi.advanceTimersByTimeAsync(0);
		expect(sp.searching).toBe(true);
		expect(sp.searchTerms).toEqual(['needle']);
		expect(api.searchSessions).toHaveBeenLastCalledWith('needle', false, 50, 0);
		sp.rawQuery = 'other';
		await vi.advanceTimersByTimeAsync(200);
		flushSync();
		await vi.advanceTimersByTimeAsync(0);
		expect(api.searchSessions).toHaveBeenLastCalledWith('other', false, 50, 0);
		expect(api.searchSessions).toHaveBeenCalledTimes(2);
	});

	it('appends the next page from the current offset', async () => {
		const { sp, api } = make();
		const full = Array.from({ length: 50 }, (_, i) => session({ id: `s${i}`, status: 'archived' }));
		api.searchSessions.mockResolvedValueOnce({ sessions: full });
		sp.sections = new Set(['archived']);
		flushSync();
		await vi.advanceTimersByTimeAsync(0);
		expect(sp.pageDone).toBe(false);
		expect(sp.pageOffset).toBe(50);
		api.searchSessions.mockResolvedValueOnce({ sessions: [session({ id: 'tail', status: 'archived' })] });
		await sp.loadPage(false);
		expect(api.searchSessions).toHaveBeenLastCalledWith('', true, 50, 50);
		expect(sp.pageRows).toHaveLength(51);
		expect(sp.pageDone).toBe(true);
	});

	it('surfaces a failed page load and bumps refreshTick to retry after archive ops', async () => {
		const { sp, api, actions } = make();
		api.searchSessions.mockRejectedValueOnce(new Error('boom'));
		sp.sections = new Set(['archived']);
		flushSync();
		await vi.advanceTimersByTimeAsync(0);
		expect(sp.pageError).toBe('boom');
		await sp.togglePin(session({ id: 'p' }));
		expect(actions.pin).toHaveBeenCalledWith('p');
		flushSync();
		await vi.advanceTimersByTimeAsync(0);
		expect(sp.pageError).toBe('');
		expect(api.searchSessions).toHaveBeenCalledTimes(2);
	});
});

describe('SessionsPage — pinned parents and the open drawer', () => {
	it('splices archived subagents of pinned parents back in and hides them from the browse', () => {
		const { sp, setItems, setAll } = make();
		setItems([session({ id: 'parent', pinned: true })]);
		setAll([
			session({ id: 'parent', pinned: true }),
			session({ id: 'kid', status: 'archived', parent_id: 'parent' })
		]);
		expect([...sp.pinnedIds]).toEqual(['parent']);
		expect(sp.pinnedArchivedKids.map((s) => s.id)).toEqual(['kid']);
		expect(sp.pinnedArchivedKidIds.has('kid')).toBe(true);
	});

	it('opens a loaded session without fetching and keeps it fresh across refetches', async () => {
		const { sp, api, setItems } = make();
		setItems([session({ id: 'a', name: 'one' })]);
		await sp.openById('a');
		expect(api.session).not.toHaveBeenCalled();
		expect(sp.openSession?.id).toBe('a');
		setItems([session({ id: 'a', name: 'renamed' })]);
		expect(sp.liveOpen?.name).toBe('renamed');
	});

	it('fetches an unknown id and drops the URL only on a 404', async () => {
		const { sp, api, deps, toasts } = make();
		await sp.openById('remote');
		expect(api.session).toHaveBeenCalledWith('remote');
		expect(sp.openSession?.id).toBe('remote');
		api.session.mockRejectedValueOnce(new Error('offline'));
		await sp.openById('flaky');
		expect(deps.clearUrlSession).not.toHaveBeenCalled();
		api.session.mockRejectedValueOnce(new ApiError(404, 'gone'));
		await sp.openById('gone');
		expect(sp.openSession).toBeNull();
		expect(deps.clearUrlSession).toHaveBeenCalledTimes(1);
		expect(toasts.error).toHaveBeenCalledTimes(2);
	});

	it('focuses the transcript hit only for a card opened while searching', async () => {
		const { sp } = make();
		const s = session({ id: 'hit', match_seq: 4 });
		sp.openFromCard(s);
		expect(sp.focusSeq).toBeNull();
		sp.rawQuery = 'term';
		await vi.advanceTimersByTimeAsync(200);
		flushSync();
		sp.openFromCard(s);
		expect(sp.focusSeq).toBe(4);
		sp.openSession = session({ id: 'other' });
		expect(sp.focusSeq).toBeNull();
	});
});

describe('SessionsPage — archive actions', () => {
	it('archives a single selection directly and leaves select mode', async () => {
		const { sp, actions, toasts } = make();
		sp.list.selecting = true;
		sp.list.selected = new Set(['a']);
		await sp.archiveSelected();
		expect(actions.archiveMany).toHaveBeenCalledWith(['a']);
		expect(toasts.ok).toHaveBeenCalled();
		expect(sp.list.selecting).toBe(false);
		expect(sp.archiveConfirm.pending).toBeNull();
	});

	it('routes a multi-selection through the confirm dialog', async () => {
		const { sp, actions } = make();
		sp.list.selected = new Set(['a', 'b']);
		await sp.archiveSelected();
		expect(actions.archiveMany).not.toHaveBeenCalled();
		expect(sp.archiveConfirm.pending?.ids).toEqual(['a', 'b']);
		await sp.archiveConfirm.confirm();
		expect(actions.archiveMany).toHaveBeenCalledWith(['a', 'b']);
	});

	it('swipe toggles by status and offers undo only for an archive', async () => {
		const { sp, actions, toasts } = make();
		await sp.swipeArchive(session({ id: 'live' }));
		expect(actions.archive).toHaveBeenCalledWith('live');
		expect(toasts.ok.mock.calls[0][2]).toBeDefined();
		await sp.swipeArchive(session({ id: 'old', status: 'archived' }));
		expect(actions.unarchive).toHaveBeenCalledWith('old');
		expect(toasts.ok.mock.calls[1][2]).toBeUndefined();
	});
});

describe('SessionsPage — spawn form and drafts', () => {
	it('opens the modal when nothing is docked, and remounts the dock otherwise', () => {
		const modal = make();
		modal.sp.openSpawn({ prompt: 'x' });
		expect(modal.sp.showSpawn).toBe(true);
		expect(modal.sp.spawnPrefill).toEqual({ prompt: 'x' });
		const docked = make({
			dockLayout: () => ({ spawn: 'right', stats: null, stacked: false, left: null, right: '30rem' })
		});
		docked.sp.openSpawn(null);
		expect(docked.sp.showSpawn).toBe(false);
		expect(docked.sp.dockEpoch).toBe(1);
	});

	it('launches and discards drafts, clearing their spawn slot', async () => {
		const { sp, actions, deps } = make();
		const d = session({ id: 'd', status: 'draft', machine_id: 'm', working_dir: '/w' });
		const launch = sp.launchDraft(d);
		expect(sp.launchingDraft).toBe('d');
		await launch;
		expect(actions.launchDraft).toHaveBeenCalledWith('d');
		expect(deps.spawnSlot.clear).toHaveBeenCalledWith('m', '/w');
		expect(sp.launchingDraft).toBeNull();
		await sp.discardDraft(d);
		expect(actions.discardDraft).toHaveBeenCalledWith('d');
	});

	it('edits a draft straight away when the form holds nothing else', () => {
		const { sp } = make();
		sp.editDraft(session({ id: 'd', status: 'draft' }));
		expect(sp.pendingDraftEdit).toBeNull();
		expect(sp.showSpawn).toBe(true);
		expect(sp.spawnPrefill?.draft_id).toBe('d');
	});

	it('asks first when the mounted form is dirty with another draft', async () => {
		const { sp, actions, deps } = make();
		const form: SpawnForm = {
			isDirty: () => true,
			currentDraftId: () => 'other',
			flushDraft: vi.fn(async () => true)
		};
		sp.spawnModal = form;
		sp.editDraft(session({ id: 'd', status: 'draft' }));
		expect(sp.pendingDraftEdit?.id).toBe('d');
		expect(sp.showSpawn).toBe(false);
		await sp.confirmDraftEdit(true);
		expect(form.flushDraft).toHaveBeenCalled();
		expect(actions.spawn).not.toHaveBeenCalled();
		expect(deps.spawnSlot.write).not.toHaveBeenCalled();
		expect(sp.pendingDraftEdit).toBeNull();
		expect(sp.spawnPrefill?.draft_id).toBe('d');
	});

	it('saves an unmounted dirty slot as a new draft before replacing it', async () => {
		const { sp, actions, deps, setSlot } = make();
		setSlot({ prompt: 'keep me', machine_id: 'm', working_dir: '/w' });
		sp.editDraft(session({ id: 'd', status: 'draft' }));
		expect(sp.pendingDraftEdit?.id).toBe('d');
		await sp.confirmDraftEdit(true);
		expect(actions.spawn).toHaveBeenCalledTimes(1);
		expect((actions.spawn.mock.calls[0] as unknown[])[0]).toMatchObject({ save_draft: true });
		expect(deps.spawnSlot.write).toHaveBeenCalledWith('slot', expect.objectContaining({ draftId: '7' }));
		expect(sp.spawnPrefill?.draft_id).toBe('d');
	});

	it('replaces without saving when asked to', async () => {
		const { sp, actions, setSlot } = make();
		setSlot({ prompt: 'keep me', machine_id: 'm', working_dir: '/w' });
		sp.editDraft(session({ id: 'd', status: 'draft' }));
		await sp.confirmDraftEdit(false);
		expect(actions.spawn).not.toHaveBeenCalled();
		expect(sp.spawnPrefill?.draft_id).toBe('d');
	});
});
