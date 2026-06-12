<script lang="ts">
	import { untrack, onMount } from 'svelte';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { useSessions, useSessionActions, endpoints, qk, SYSTEM_MACHINE_KINDS } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { page } from '$app/state';
	import { pushState, replaceState } from '$app/navigation';
	import { toasts } from '$lib/toast.svelte';
	import { ApiError } from '$lib/api';
	import { ws } from '$lib/ws.svelte';
	import SessionCard from '$lib/components/SessionCard.svelte';
	import SubagentBadge from '$lib/components/SubagentBadge.svelte';
	import ConversationDrawer from '$lib/components/ConversationDrawer.svelte';
	import SpawnModal from '$lib/components/SpawnModal.svelte';
	import Button from '$lib/components/Button.svelte';
	import IconButton from '$lib/components/IconButton.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import { drafts, LIST_DENSITY, DISPATCHED_COLLAPSED, LIST_VIEW, LIST_SECTION } from '$lib/drafts';
	import { notify } from '$lib/notify.svelte';
	import { tokenizeQuery } from '$lib/search';

	// Close a popover when a pointer/focus lands outside the node it's attached to.
	function clickOutside(node: HTMLElement, onOutside: () => void) {
		const handler = (e: Event) => {
			if (!node.contains(e.target as Node)) onOutside();
		};
		document.addEventListener('pointerdown', handler, true);
		return {
			destroy() {
				document.removeEventListener('pointerdown', handler, true);
			}
		};
	}

	let dense = $state(drafts.get(LIST_DENSITY) === 'compact');
	$effect(() => {
		drafts.set(LIST_DENSITY, dense ? 'compact' : 'normal');
	});

	// Main-list layout × density semantics (CCT-305). Two independent toggles:
	//   list ⇄ grid (cardView) and compact ⇄ detailed (dense). The 2×2 matrix:
	//     list  + compact  → one row per session
	//     list  + detailed → multi-row per session
	//     grid  + compact  → grid of cards constrained to the list container width
	//                        (~2 columns), NOT full-bleed
	//     grid  + detailed → the SAME cards, no column cap → spans the full window
	//   Card MARKUP is identical across compact/detailed in grid (always the
	//   detailed card); only the container width / column cap changes. Grid is
	//   top-level only (subagents stay in list view / the drawer).
	let cardView = $state(drafts.get(LIST_VIEW) === 'card');
	$effect(() => {
		drafts.set(LIST_VIEW, cardView ? 'card' : 'list');
	});

	// View picker (CCT-307): collapse the two awkward icon toggles (which still
	// overflowed the toolbar) into one labelled picker offering the 4 explicit
	// combinations. Persistence is unchanged — the picker just reads/writes the
	// existing `cardView` (list ⇄ card) and `dense` (compact ⇄ detailed) state,
	// each of which already round-trips through drafts above.
	const VIEW_OPTIONS = [
		{ value: 'list-compact', label: 'List · Compact', card: false, dense: true },
		{ value: 'list-detailed', label: 'List · Detailed', card: false, dense: false },
		{ value: 'card-compact', label: 'Card · Compact', card: true, dense: true },
		{ value: 'card-detailed', label: 'Card · Detailed', card: true, dense: false }
	] as const;
	let viewMode = $derived(`${cardView ? 'card' : 'list'}-${dense ? 'compact' : 'detailed'}`);
	function selectView(value: string) {
		const opt = VIEW_OPTIONS.find((o) => o.value === value);
		if (!opt) return;
		cardView = opt.card;
		dense = opt.dense;
	}
	let viewLabel = $derived(VIEW_OPTIONS.find((o) => o.value === viewMode)?.label ?? 'View');

	// Collapse/hide the Dispatched group as one unit (CCT-279 item 6). Persisted.
	let dispatchedCollapsed = $state(drafts.get(DISPATCHED_COLLAPSED) === '1');
	$effect(() => {
		drafts.set(DISPATCHED_COLLAPSED, dispatchedCollapsed ? '1' : '0');
	});

	// Section filter (CCT-322 / CCT-345): the sessions list is partitioned into
	// four sections, each an INDEPENDENT on/off toggle (not a forced single
	// choice) — one toolbar button opens a popover of four checkboxes so any
	// combination can be shown at once. Semantics over the loaded list:
	//   • starred    → pinned sessions (CCT-267)
	//   • live       → interactive, non-dispatched, non-pinned sessions
	//   • dispatched → server-managed / ephemeral-worker sessions (CCT-231)
	//   • archived   → also append the paginated archive browse (CCT-184)
	// The chosen set is persisted (comma-joined) so it sticks across reloads.
	type Section = 'starred' | 'live' | 'dispatched' | 'archived';
	const SECTIONS: { value: Section; label: string; icon: 'star' | 'live' | 'send' | 'archive' }[] = [
		{ value: 'starred', label: 'Starred', icon: 'star' },
		{ value: 'live', label: 'Live', icon: 'live' },
		{ value: 'dispatched', label: 'Dispatched', icon: 'send' },
		{ value: 'archived', label: 'Archived', icon: 'archive' }
	];
	const isSection = (v: string): v is Section =>
		v === 'starred' || v === 'live' || v === 'dispatched' || v === 'archived';
	const parseSections = (raw: string | null): Set<Section> => {
		const set = new Set<Section>((raw ?? '').split(',').filter(isSection));
		// Never strand the user on an empty list (would render nothing).
		return set.size ? set : new Set<Section>(['starred', 'live', 'dispatched']);
	};
	let sections = $state<Set<Section>>(parseSections(drafts.get(LIST_SECTION)));
	let sectionMenuOpen = $state(false);
	function toggleSection(v: Section) {
		const next = new Set(sections);
		if (next.has(v)) next.delete(v);
		else next.add(v);
		if (next.size === 0) return; // keep at least one on
		sections = next;
	}
	$effect(() => {
		drafts.set(LIST_SECTION, [...sections].join(','));
	});
	const sectionCount = $derived(sections.size);
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

	// Workflow-tool subagents (CCT-225) carry a `workflow_run_id` in their
	// session metadata. A single run can spawn 100+ agents, so rather than
	// dumping them all as flat children we group them by run id under a
	// collapsible "Workflow: <name> (<runId>)" header. Plain (Task-tool)
	// children render as before. Returns, per parent id, the ungrouped children
	// plus an ordered list of workflow groups.
	// A subagent group folded under a parent. Workflow-tool subagents (CCT-225)
	// carry a `workflow_run_id`; plain (Task-tool) children share the synthetic
	// "plain" group. Each group renders inline (always expanded) when it has
	// fewer than 3 agents; larger groups collapse behind a count badge on the
	// parent row that toggles expand/collapse (CCT-269).
	type SubGroup = {
		// Stable key, unique within a parent: "plain" or "wf:<runId>".
		key: string;
		// Run id for workflow groups; null for the plain group.
		runId: string | null;
		// Tooltip label, e.g. "Workflow: deploy" or "subagents".
		label: string;
		agents: SessionListItem[];
		running: number;
	};
	const INLINE_THRESHOLD = 3; // < this → always expanded inline, no badge
	function metaStr(s: SessionListItem, key: string): string | null {
		const m = s.metadata as Record<string, unknown> | null;
		const v = m?.[key];
		return typeof v === 'string' ? v : null;
	}
	const relationOf = (s: SessionListItem) => metaStr(s, 'relation') ?? (metaBool(s, 'subagent') ? 'subagent' : 'root');
	function metaBool(s: SessionListItem, key: string): boolean {
		const m = s.metadata as Record<string, unknown> | null;
		return m?.[key] === true;
	}
	const runningCount = (agents: SessionListItem[]) =>
		agents.filter((a) => a.status !== 'archived' && a.liveness !== 'dead' && !a.hibernated).length;
	// Fold a parent's children into plain + per-workflow groups.
	function groupChildren(kids: SessionListItem[]): SubGroup[] {
		const plain: SessionListItem[] = [];
		const byRun = new Map<string, { name: string | null; agents: SessionListItem[] }>();
		for (const k of kids) {
			const runId = metaStr(k, 'workflow_run_id');
			if (runId) {
				let g = byRun.get(runId);
				if (!g) {
					g = { name: metaStr(k, 'workflow_name'), agents: [] };
					byRun.set(runId, g);
				}
				g.agents.push(k);
			} else {
				plain.push(k);
			}
		}
		const groups: SubGroup[] = [];
		if (plain.length > 0) {
			groups.push({
				key: 'plain',
				runId: null,
				label: 'subagents',
				agents: plain,
				running: runningCount(plain)
			});
		}
		for (const [runId, g] of byRun) {
			groups.push({
				key: `wf:${runId}`,
				runId,
				label: g.name ? `Workflow: ${g.name}` : 'Workflow',
				agents: g.agents,
				running: runningCount(g.agents)
			});
		}
		return groups;
	}

	// Build the parent→subagent-group nesting for an arbitrary row set. Used for
	// the live buckets AND, since CCT-298 item 1, for the archive + search views
	// so they keep the same nesting + count badges instead of a flat list. A
	// child whose parent is absent from `rows` falls back to top-level so nothing
	// is dropped.
	type Nest = {
		topLevel: SessionListItem[];
		childGroups: Map<string, SubGroup[]>;
		hasCollapsible: boolean;
	};
	function nest(rows: SessionListItem[]): Nest {
		const ids = new Set(rows.map((s) => s.id));
		const childrenOf = new Map<string, SessionListItem[]>();
		for (const s of rows) {
			if (s.parent_id && ids.has(s.parent_id) && relationOf(s) !== 'fork') {
				childrenOf.set(s.parent_id, [...(childrenOf.get(s.parent_id) ?? []), s]);
			}
		}
		const topLevel = rows.filter((s) => !s.parent_id || !ids.has(s.parent_id) || relationOf(s) === 'fork');
		const childGroups = new Map<string, SubGroup[]>();
		let hasCollapsible = false;
		for (const [parentId, kids] of childrenOf) {
			const groups = groupChildren(kids);
			if (groups.length > 0) childGroups.set(parentId, groups);
			if (groups.some((g) => g.agents.length >= INLINE_THRESHOLD)) hasCollapsible = true;
		}
		return { topLevel, childGroups, hasCollapsible };
	}

	// Aggregated subagent usage for a parent (CCT-297 #19, tokens per CCT-301 #2):
	// the parent's own total tokens plus every subagent's, with the agent count.
	// Reported in tokens (not dollars). Null when there are no agents.
	const totalTokens = (u: SessionListItem['token_usage']) =>
		Number(u.tokens_in) +
		Number(u.tokens_out) +
		Number(u.cache_read_tokens) +
		Number(u.cache_creation_tokens);
	function costRollup(
		s: SessionListItem,
		groups: SubGroup[]
	): { tokens: number; count: number } | null {
		const agents = groups.flatMap((g) => g.agents);
		if (agents.length === 0) return null;
		const tokens =
			totalTokens(s.token_usage) + agents.reduce((n, a) => n + totalTokens(a.token_usage), 0);
		return { tokens, count: agents.length };
	}

	const liveNest = $derived(nest(items));
	const topLevel = $derived(liveNest.topLevel);
	const childGroupsOf = $derived(liveNest.childGroups);
	const hasCollapsibleSubagents = $derived(liveNest.hasCollapsible);

	// Expand/collapse state for collapsible (>=3) subagent groups, keyed by
	// `${parentId}/${group.key}`. Default collapsed.
	let expanded = $state(new Set<string>());
	const groupId = (parentId: string, key: string) => `${parentId}/${key}`;
	function toggleGroup(parentId: string, key: string) {
		const id = groupId(parentId, key);
		const next = new Set(expanded);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		expanded = next;
	}

	// Classifier buckets (CCT-90), in attention-first display order. Sessions
	// that want the user's eyes float to the top; empty buckets are dropped.
	// Sessions on server-managed machines (dispatch / ephemeral workers) get
	// their own "Dispatched" group at the bottom (CCT-231) — they're unattended
	// noise next to interactive sessions — EXCEPT blocked ones, which still
	// surface under Needs input so attention never gets buried.
	type GroupKey = SessionListItem['bucket'] | 'dispatched' | 'pinned';
	const BUCKETS: { key: GroupKey; label: string }[] = [
		// Pinned/starred sessions (CCT-267) float above every bucket.
		{ key: 'pinned', label: 'Pinned' },
		{ key: 'blocked', label: 'Needs input' },
		{ key: 'review', label: 'Ready for review' },
		{ key: 'working', label: 'Working' },
		{ key: 'done', label: 'Completed' },
		{ key: 'dispatched', label: 'Dispatched' }
	];
	const isDispatched = (s: SessionListItem) =>
		s.machine_kind != null && SYSTEM_MACHINE_KINDS.has(s.machine_kind);
	const groupOf = (s: SessionListItem): GroupKey => {
		if (s.pinned) return 'pinned';
		const bucket = s.bucket ?? 'working';
		if (bucket === 'blocked') return 'blocked';
		return isDispatched(s) ? 'dispatched' : bucket;
	};
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
	<IconButton class="search-toggle btn-control-square" icon="search" label="Search chats" onclick={openSearch} />
	<div class="search-wrap" class:open={searchOpen}>
		<input
			class="search"
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
		{#if rawQuery}
			<!-- In-field clear (CCT-297 #22). onmousedown preventDefault keeps the
			     input from blurring (which on mobile would collapse it) before the
			     click clears the query. -->
			<button
				type="button"
				class="search-clear"
				aria-label="Clear search"
				title="Clear search"
				onmousedown={(e) => e.preventDefault()}
				onclick={() => {
					rawQuery = '';
					searchEl?.focus();
				}}>✕</button
			>
		{/if}
	</div>
	<!-- Section filter (CCT-322 / CCT-345): ONE button opens a popover of four
	     independent on/off toggles (Starred / Live / Dispatched / Archived) so any
	     combination can be shown at once — not a forced single choice. -->
	<div
		class="section-pick"
		use:clickOutside={() => (sectionMenuOpen = false)}
	>
		<IconButton
			class="btn-control-square"
			icon="filter"
			label="Filter sections"
			title="Filter sections ({sectionCount}/4)"
			aria-haspopup="true"
			aria-expanded={sectionMenuOpen}
			aria-pressed={sectionCount < SECTIONS.length}
			onclick={() => (sectionMenuOpen = !sectionMenuOpen)}
		/>
		{#if sectionCount < SECTIONS.length}<span class="section-count" aria-hidden="true">{sectionCount}</span>{/if}
		{#if sectionMenuOpen}
			<div class="section-menu" role="menu" aria-label="Sections">
				{#each SECTIONS as sec (sec.value)}
					<button
						type="button"
						role="menuitemcheckbox"
						class="section-opt"
						aria-checked={sections.has(sec.value)}
						onclick={() => toggleSection(sec.value)}
					>
						<span class="section-check" aria-hidden="true">{sections.has(sec.value) ? '✓' : ''}</span>
						<Icon name={sec.icon} size={15} />
						<span class="section-opt-label">{sec.label}</span>
					</button>
				{/each}
			</div>
		{/if}
	</div>
	<!-- View picker (CCT-307): one labelled control offering the 4 explicit
	     layout × density combinations, replacing the two icon toggles that still
	     overflowed the toolbar. A native <select> overlaid transparently on the
	     trigger button (same pattern as the header theme/font pickers) gives the
	     platform popup with zero outside-click bookkeeping. -->
	<div class="view-pick btn-control" title="View: {viewLabel}" aria-label="View: {viewLabel}">
		<span class="view-pick-icon" aria-hidden="true">{cardView ? '▦' : '☰'}</span>
		<span class="view-pick-caret" aria-hidden="true">▾</span>
		<select
			aria-label="Choose list view"
			value={viewMode}
			onchange={(e) => selectView((e.currentTarget as HTMLSelectElement).value)}
		>
			{#each VIEW_OPTIONS as o (o.value)}
				<option value={o.value}>{o.label}</option>
			{/each}
		</select>
	</div>
	{#if !searching}
		{#if selecting}
			<Button class="toolbar-select" control title="Cancel selection" aria-label="Cancel selection" onclick={exitSelect}>✕</Button>
		{:else}
			<Button class="toolbar-select" control title="Select multiple to archive" aria-label="Select multiple to archive" onclick={() => (selecting = true)}>☑</Button>
		{/if}
	{/if}
	<Button class="toolbar-new" control variant="primary" onclick={() => (showSpawn = true)}>+ New</Button>
</div>

{#if selecting && !searching}
		<div class="bulkbar row">
			<span class="count">{selected.size} selected</span>
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
{#snippet cardGrid(rows: SessionListItem[])}
	<!-- Grid (CCT-305 / CCT-321): the card uses the SAME canonical field model in
	     both densities; `compact` (dense) only tightens spacing + clamps the
	     preview to one line, and the grid narrows to a 2-column cap. Detailed cards
	     are deliberately the largest (wider track + roomier card). `grid` keeps a
	     uniform single-line cwd path so a row of cards stays the same height. -->
	<div class="grid" class:narrow={dense}>
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
			/>
		{/each}
	</div>
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
			{#if collapsibleGroups.length > 0}
				<div class="subagent-badge-rail">
					{#each collapsibleGroups as g (g.key)}
						<SubagentBadge
							count={g.agents.length}
							running={g.running}
							open={expanded.has(groupId(s.id, g.key))}
							label={g.label}
							ontoggle={() => toggleGroup(s.id, g.key)}
						/>
					{/each}
				</div>
			{/if}
			<div class="parent-card">
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
				/>
			</div>
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
		<div class="empty err">Search failed: {pageError}</div>
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
		<div class="empty">No chats match “{query}”{showArchived ? '' : ' (live only — pick Archived to search all)'}.</div>
	{:else}
		<!-- Nest over the whole result set so a parent and its subagents stay
		     grouped even if they land in different status sections; then split
		     the top-level rows into Live / Archived (CCT-298 item 1). -->
		{@const ns = nest(pageRows)}
		{@const liveTop = ns.topLevel.filter((s) => s.status !== 'archived')}
		{@const archTop = ns.topLevel.filter((s) => s.status === 'archived')}
		<div class="stack" class:tight={dense} class:badge-gutter={ns.hasCollapsible}>
			{#if liveTop.length > 0}
				<div class="group-header">Live <span class="count">{liveTop.length}</span></div>
				{@render nestedRows(liveTop, ns.childGroups, false, searchTerms)}
			{/if}
			{#if archTop.length > 0}
				<div class="group-header">Archived <span class="count">{archTop.length}</span></div>
				{@render nestedRows(archTop, ns.childGroups, false, searchTerms)}
			{/if}
			{@render loadMore()}
		</div>
	{/if}
{:else}
	<!-- Live buckets first… -->
	{#if $sessions.isLoading}
		<div class="empty"><span class="spin"></span></div>
	{:else if groups.length === 0 && !showArchived}
		<div class="empty">
			No sessions in the selected sections — toggle more from the section filter.
		</div>
	{:else}
		<div
			class="stack"
			class:tight={dense && !cardView}
			class:badge-gutter={!cardView && hasCollapsibleSubagents}
		>
			{#each groups as g (g.key)}
				{#if g.key === 'dispatched'}
					<!-- Dispatched group (CCT-279 items 6 + 7): collapsible as one unit
					     with a bulk "Archive all" action. -->
					<div class="group-header" data-bucket={g.key}>
						<button
							class="group-toggle"
							aria-expanded={!dispatchedCollapsed}
							onclick={() => (dispatchedCollapsed = !dispatchedCollapsed)}
						>
							<span class="chev" class:collapsed={dispatchedCollapsed}>▾</span>
							{g.label} <span class="count">{g.sessions.length}</span>
						</button>
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
						<div class="spacer"></div>
						</div>
				{:else}
					<div class="group-header" data-bucket={g.key}>
						{g.label} <span class="count">{g.sessions.length}</span>
					</div>
				{/if}
				{@const vis = g.key === 'dispatched' && dispatchedCollapsed ? [] : g.sessions}
				{#if cardView}
					{@render cardGrid(vis)}
				{:else}
					{@render nestedRows(vis, childGroupsOf, true, [])}
				{/if}
			{/each}
		</div>
	{/if}

	<!-- …then the paginated archive when requested. -->
	{#if showArchived}
		{@const ns = nest(pageRows)}
		<div class="stack" class:tight={dense} class:badge-gutter={ns.hasCollapsible}>
			<div class="group-header">Archived <span class="count">{ns.topLevel.length}</span></div>
			{#if pageRows.length === 0 && !pageLoading}
				<div class="empty">No archived sessions.</div>
			{:else if cardView}
				<!-- Card mode applies to archived sessions too (CCT-321 parity). -->
				{@render cardGrid(ns.topLevel)}
				{@render loadMore()}
			{:else}
				{@render nestedRows(ns.topLevel, ns.childGroups, false, searchTerms)}
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
	.bar {
		/* Sticky under the fixed app header so search/density/New stay reachable
		   on long lists without scrolling back up (CCT-241). Also the positioning
		   context for the mobile search overlay. */
		position: sticky;
		top: calc(var(--header-h) + var(--safe-top));
		z-index: 6;
		margin-bottom: var(--sp-4);
		/* CCT-314: only pad the bottom. The earlier symmetric `var(--sp-2) 0`
		   top-padding pushed the title ~8px lower than every other page's header
		   (which have no sticky top padding), so Sessions looked misaligned. */
		padding: 0 0 var(--sp-2);
		gap: var(--sp-2);
		/* CCT-250 item 1: center all toolbar controls on one baseline so the
		   magnifier button lines up with the other buttons (was `stretch`,
		   which only stretched text buttons → the icon-only search button sat
		   at a different height). */
		align-items: center;
		/* CCT-308 item 3: the toolbar is a no-wrap flex row whose buttons are
		   `flex:none` + `white-space:nowrap` (they can't shrink). Bumping the UI
		   scale grew the title + button text past the viewport width; since the
		   page clips horizontal overflow, the controls were pushed off-frame and
		   unreachable. Let the toolbar wrap so controls reflow onto a second line
		   instead of overflowing, keeping the title + every action in-frame at any
		   scale. */
		flex-wrap: wrap;
		background: var(--bg);
	}
	.page-title {
		/* CCT-308 item 3: the title is toolbar chrome, not content — pin it to a
		   fixed px size (1.75rem @16px = 28px) so the UI font scale doesn't grow
		   it and shove the action buttons out of frame. */
		font-size: 28px;
		align-self: center;
		flex: none;
	}
	/* View picker (CCT-307): a native <select> overlaid transparently on a
	   labelled trigger (same pattern as the header theme/font pickers) so it gets
	   the platform popup with no custom outside-click logic. One control instead
	   of two icon toggles, so the toolbar no longer overflows. */
	.view-pick {
		position: relative;
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		flex: none;
		white-space: nowrap;
		cursor: pointer;
	}
	.view-pick-caret {
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.view-pick select {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		opacity: 0;
		cursor: pointer;
		border: none;
		background: none;
	}
	/* Section filter (CCT-345): one square toolbar button that opens a popover of
	   independent toggles. The wrapper is the positioning context for the popover
	   and the small "active count" badge. */
	.section-pick {
		position: relative;
		display: inline-flex;
		align-items: center;
		flex: none;
		align-self: center;
	}
	/* Count badge shown when not all four sections are enabled (i.e. a filter is
	   active), so the toolbar shows at a glance that the list is filtered. */
	.section-count {
		position: absolute;
		top: -0.35rem;
		right: -0.35rem;
		min-width: 1rem;
		height: 1rem;
		padding: 0 0.25rem;
		border-radius: 999px;
		background: var(--accent);
		color: var(--bg);
		font-size: 0.62rem;
		font-weight: var(--fw-semibold);
		line-height: 1rem;
		text-align: center;
		pointer-events: none;
	}
	.section-menu {
		position: absolute;
		top: calc(100% + var(--sp-1));
		right: 0;
		z-index: 40;
		min-width: 12rem;
		display: flex;
		flex-direction: column;
		padding: var(--sp-1);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.4));
	}
	.section-opt {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		width: 100%;
		padding: var(--sp-2);
		border: none;
		border-radius: var(--r-sm);
		background: none;
		color: var(--text);
		font-size: var(--fs-sm);
		text-align: left;
		cursor: pointer;
	}
	.section-opt:hover {
		background: var(--bg-hover, var(--border));
	}
	.section-opt[aria-checked='true'] {
		color: var(--accent, var(--text));
	}
	.section-check {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.05rem;
		height: 1.05rem;
		flex: none;
		border-radius: var(--r-sm);
		border: 1.5px solid var(--border-strong);
		font-size: 0.75rem;
		line-height: 1;
	}
	.section-opt[aria-checked='true'] .section-check {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--bg);
	}
	.section-opt-label {
		flex: 1 1 auto;
	}
	/* Fill all the space between the title and the Archived checkbox; the wrapper
	   is the flex item / positioning context for the in-field clear button. */
	.search-wrap {
		flex: 1;
		min-width: 0;
		position: relative;
		display: flex;
	}
	.search {
		flex: 1;
		min-width: 0;
		height: var(--control-height);
		/* room on the right for the clear button so text doesn't run under it */
		padding: var(--sp-1) calc(var(--sp-3) + 1.25rem) var(--sp-1) var(--sp-3);
		font-size: var(--fs-sm);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		color: var(--text);
	}
	/* Hide any browser-native search clear; we provide our own cross. */
	.search::-webkit-search-cancel-button {
		display: none;
	}
	.search-clear {
		position: absolute;
		top: 50%;
		right: var(--sp-2);
		transform: translateY(-50%);
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.25rem;
		height: 1.25rem;
		padding: 0;
		border: none;
		border-radius: 50%;
		background: var(--bg-elevated-2, var(--border));
		color: var(--text-faint);
		font-size: var(--fs-xs);
		line-height: 1;
		cursor: pointer;
	}
	.search-clear:hover {
		color: var(--text);
	}
	/* Desktop: the input always fills the bar; no toggle button. */
	:global(.search-toggle) {
		display: none;
		align-self: center;
	}
	/* Mobile: collapsed to a magnifier; tapping it expands the input over the
	   whole toolbar with a smooth width/opacity transition (CCT-241). */
	@media (max-width: 639px) {
		.bar {
			display: grid;
			grid-template-columns: auto minmax(0, 1fr) auto;
			align-items: center;
		}
		.bar > .page-title {
			grid-column: 1;
		}
		.bar > :global(.search-toggle),
		.bar > .search-wrap {
			grid-column: 2;
		}
		.bar > :global(.toolbar-new) {
			grid-column: 3;
			grid-row: 1;
		}
		/* Second row (CCT-322 mobile layout): section picker · view toggle · select
		   toggle — the icon-only controls pack to the left instead of stretching
		   across the wide 1fr search column (CCT-345). */
		.section-pick,
		.view-pick,
		.bar > :global(.toolbar-select) {
			grid-row: 2;
			justify-self: start;
		}
		.section-pick {
			grid-column: 1;
		}
		.view-pick {
			grid-column: 2;
			justify-self: start;
		}
		.bar > :global(.toolbar-select) {
			grid-column: 3;
		}
		:global(.search-toggle) {
			display: none;
		}
		.search-wrap {
			position: static;
			width: 100%;
			opacity: 1;
			pointer-events: auto;
		}
		.search-wrap.open {
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
	/* Grid (card) view (CCT-305). Two widths driven by the density toggle:
	   - detailed (default): full-bleed — escape the centered --content-max column
	     and span the whole viewport with as many uniform cards as fit.
	   - compact (.narrow): stay inside the normal list container width with a
	     ~2-column cap, so it reads as a tighter version of the list rather than a
	     full-window sprawl.
	   Card MARKUP is identical between the two (always the detailed card); only
	   the container width and the column template change. `align-items: stretch`
	   makes every card in a row the same height so rows aren't ragged. */
	.grid {
		display: grid;
		gap: var(--sp-3);
		align-items: stretch;
		/* full-bleed: escape the .container max-width and span the viewport */
		width: 100vw;
		position: relative;
		left: 50%;
		margin-left: -50vw;
		box-sizing: border-box;
		/* Detailed grid: as many columns as fit at a column width tuned for cards
		   that read more vertical than horizontal (CCT-305 follow-up) — a narrower
		   track than the old 20rem packs more columns across a wide window while
		   keeping each card a portrait-ish rectangle. */
		/* Detailed = the spacious view: a WIDE track so few, large cards fill the
		   row. Must read bigger than compact (CCT-345 follow-up). */
		grid-template-columns: repeat(auto-fill, minmax(34rem, 1fr));
		padding-inline: max(var(--sp-4), var(--safe-left)) max(var(--sp-4), var(--safe-right));
	}
	/* Compact grid: denser than detailed — also full-bleed, but a NARROWER track so
	   more, smaller cards pack across the row. minmax(0,1fr) lets cards shrink on
	   narrow viewports; a single column kicks in below ~32rem via the media query. */
	.grid.narrow {
		gap: var(--sp-2);
		grid-template-columns: repeat(auto-fill, minmax(21rem, 1fr));
	}
	/* Mobile card grid (CCT-305 follow-up). The two densities now diverge on phones,
	   mirroring desktop's "compact = more in less space, detailed = more per card":
	   - compact (.grid.narrow) → 2 narrow columns, each card half-viewport wide so
	     it reads as a proper (vertical) card rather than a wide list row.
	   - detailed (.grid) → a single full-viewport-width column, each card at least
	     ~half the screen tall, giving a squarish-to-slightly-vertical aspect with
	     plenty of room for the 3-line message preview.
	   (The detailed-grid minmax(17rem) would otherwise resolve to a single column
	   below ~36rem anyway, but we pin it explicitly and add the min-height.) */
	@media (max-width: 639px) {
		.grid {
			grid-template-columns: 1fr;
			gap: var(--sp-2);
		}
		/* CCT-312: the old `min-height: 48vh` forced every detailed mobile card to
		   half the screen even when sparse, so cards read as "big and empty". Let
		   them size to their content (still floored by the base 11rem) and let the
		   message preview fill/cap the height instead (see `.last` in SessionCard). */
		.grid:not(.narrow) :global(.sc.grid .preview) {
			max-height: 42vh;
		}
		.grid.narrow {
			grid-template-columns: repeat(2, minmax(0, 1fr));
			gap: var(--sp-2);
		}
	}
	/* Parent row (CCT-269): the card remains a normal full-width row. Count
	   badge(s) are absolutely positioned before it so they don't affect row
	   sizing or the alignment of card contents. */
	.parent-row {
		position: relative;
	}
	.parent-row .parent-card {
		width: 100%;
		min-width: 0;
	}
	.subagent-badge-rail {
		position: absolute;
		z-index: 2;
		top: var(--sp-4);
		right: calc(100% + var(--sp-2));
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: var(--sp-1);
	}
	.parent-row.dense .subagent-badge-rail {
		top: var(--sp-2);
	}
	.agent-children {
		margin-left: min(calc(var(--agent-depth) * var(--sp-4)), 5rem);
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	@media (max-width: 639px) {
		.stack.badge-gutter {
			padding-left: calc(1.5rem + var(--sp-2));
		}
		.agent-children {
			margin-left: min(calc(var(--agent-depth) * var(--sp-2)), 2.5rem);
		}
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
	/* Dispatched group collapse toggle (CCT-279 item 6). */
	.group-toggle {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		background: none;
		border: none;
		padding: 0;
		font: inherit;
		color: inherit;
		text-transform: inherit;
		letter-spacing: inherit;
		cursor: pointer;
	}
	.group-toggle:hover {
		color: var(--text);
	}
	.group-toggle .chev {
		display: inline-block;
		transition: transform 0.12s var(--ease);
		font-size: 0.85em;
	}
	.group-toggle .chev.collapsed {
		transform: rotate(-90deg);
	}
	.group-header[data-bucket='blocked'] {
		color: var(--warn, #d08770);
	}
	.group-header[data-bucket='review'] {
		color: var(--accent, #88c0d0);
	}
</style>
