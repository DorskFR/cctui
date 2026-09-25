<script lang="ts">
	import { untrack, onMount } from 'svelte';
	import {
		useSessions,
		useSessionActions,
		useInvalidateSessions,
		useLabels,
		useAllMachines,
		endpoints,
		qk
	} from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { page } from '$app/state';
	import { pushState, replaceState } from '$app/navigation';
	import { toasts } from '$lib/toast.svelte';
	import { ws } from '$lib/ws.svelte';
	import ConversationDrawer from '$lib/components/organisms/ConversationDrawer.svelte';
	import SpawnModal from '$lib/components/organisms/SpawnModal.svelte';
	import { dockLayout } from '$lib/spawnDock.svelte';
	import StatsDock from '$lib/components/organisms/statsdock/StatsDock.svelte';
	import SessionControls from '$lib/components/organisms/SessionControls.svelte';
	import { ConfirmModal } from '@dorsk/tsumikit';
	import SessionSections from './SessionSections.svelte';
	import SessionsBulkBar from './SessionsBulkBar.svelte';
	import EditDraftModal from './EditDraftModal.svelte';
	import { drafts, clearSpawnSlot, currentSpawnSlot, readSpawnSlot } from '$lib/drafts';
	import { notify } from '$lib/notify.svelte';
	import { settings } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';
	import { sessionIdFromLocation, sessionHrefFor, toGroupDimension } from './sessions.logic';
	import { SessionsPage } from './sessionsPage.svelte';

	// Live buckets always show non-archived sessions; the archive is a separate
	// paginated section below.
	const sessions = useSessions(() => false);
	const qc = useQueryClient();
	const invalidateSessions = useInvalidateSessions();
	const actions = useSessionActions();
	const labelsQuery = useLabels();

	// Visual order of the list, read from the DOM: rows are rendered by a
	// recursive snippet across several buckets, so document order is the only
	// place the flattened order the user actually sees exists.
	function renderedIds(): string[] {
		return [...document.querySelectorAll<HTMLElement>('[data-session-id]')]
			.map((el) => el.dataset.sessionId!)
			.filter((id) => id);
	}

	const sp = new SessionsPage({
		store: drafts,
		settings,
		toasts,
		api: endpoints,
		actions,
		invalidateSessions,
		refetchLive: () => qc.invalidateQueries({ queryKey: qk.sessions(false) }),
		dockLayout,
		clearUrlSession: () => setUrlSession(null, true),
		confirm: (message) => confirm(message),
		spawnSlot: {
			currentKey: currentSpawnSlot,
			read: readSpawnSlot,
			write: (key, payload) => drafts.set(key, JSON.stringify(payload)),
			clear: clearSpawnSlot
		},
		sessions: () => sessions.data?.sessions ?? [],
		sessionsLoading: () => sessions.isLoading,
		allSessions: () => allSessions.data?.sessions ?? [],
		labels: () => labelsQuery.data?.labels,
		renderedOrder: renderedIds
	});
	// The full (incl. archived) list, only fetched while a pinned parent may
	// have archived subagents to splice back under it.
	const allSessions = useSessions(
		() => true,
		() => sp.pinnedIds.size > 0
	);

	// Machine liveness for the group headers; falls back to "unknown" (no dot)
	// when the machines endpoint is not readable.
	const machines = useAllMachines(() => sp.groupBy === 'machine');
	const machineLiveness = (name: string): 'online' | 'stale' | 'offline' | null =>
		(machines.data ?? []).find((mc) => mc.name === name)?.liveness ?? null;

	// ── Deep-linkable session ─────────────────────────────────────
	// A session's stable, shareable URL is /sessions?session=<id>. The whole SPA
	// already sits behind the login wall (layout renders <Login/> when unauthed
	// and keeps the URL intact), so following a shared link while logged out shows
	// the login wall and lands on the requested session right after auth — the
	// "return to intended destination" is free as long as we never redirect away.
	//
	// `sp.openSession` is the source of truth for the drawer; the URL mirrors it.
	// We track the last id we synced to the URL so list refetches (which churn
	// the session object) don't re-push.
	let lastUrlId: string | null = null;
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
		if (id === untrack(() => sp.openSession?.id ?? null)) return;
		if (id) void sp.openById(id);
		else sp.openSession = null;
	});

	// drawer → URL: reflect the open session into the address bar so it's always
	// a shareable link. Skip while we're resolving a URL-driven open (no echo).
	$effect(() => {
		const id = sp.openSession?.id ?? null;
		if (sp.urlResolving) return;
		if (!mounted) return;
		if (id === lastUrlId) return;
		// Always push so opening/closing a session is a real history step and Back
		// returns to the list. The deep-link case is handled by seeding lastUrlId
		// on mount (above), so no redundant entry is added on first load.
		setUrlSession(id, false);
		lastUrlId = id;
	});

	// live status changes from the websocket → refetch the list
	$effect(() => {
		void ws.changeTick;
		invalidateSessions();
	});

	// Tell the notifier which drawer is open so it won't notify for it.
	$effect(() => {
		notify.openSessionId = sp.openSession?.id ?? null;
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
		const id = sp.openSession?.id ?? null;
		if (!id) return;
		void actions
			.markSeen(id)
			.then(() => invalidateSessions())
			.catch(() => {});
	});

	// A clicked notification asks us to open its session's drawer; `openById`
	// falls back to fetching it when it's in no loaded list.
	$effect(() => {
		const id = notify.pendingOpen;
		if (!id) return;
		void sp.openById(id);
		notify.pendingOpen = null;
	});

	const pending = (id: string) => {
		void ws.changeTick; // re-derive when perms change (setPerms bumps changeTick)
		return ws.pendingCount(id);
	};
</script>


<SessionControls
	bind:rawQuery={sp.rawQuery}
	searchSchema={sp.searchSchema}
	bind:sections={sp.sections}
	labels={sp.allLabels}
	bind:labelFilter={sp.labelFilter}
	bind:cardView={sp.cardView}
	colorBy={sp.colorBy}
	groupBy={sp.groupBy}
	onColorBy={sp.setColorBy}
	onGroupBy={(v) => settings.setSessionList({ groupBy: toGroupDimension(v) })}
	selecting={sp.list.selecting}
	searching={sp.searching}
	onStartSelect={() => (sp.list.selecting = true)}
	onCancelSelect={sp.list.exitSelect}
	onNew={sp.dockSide ? undefined : () => (sp.showSpawn = true)}
	onUpdateLabel={sp.updateLabel}
	onDeleteLabel={sp.deleteLabel}
/>

{#if sp.list.selecting}
	<SessionsBulkBar {sp} />
{/if}

<SessionSections {sp} {pending} {machineLiveness} />

{#if sp.liveOpen}
	<ConversationDrawer
		session={sp.liveOpen}
		onclose={() => (sp.openSession = null)}
		highlight={sp.searchTerms}
		focusSeq={sp.focusSeq}
		onNewFromScript={sp.newFromScript}
		onFollowup={sp.followUp}
		onNavigate={(sid) => void sp.navigateToForked(sid)}
	/>
{/if}

{#if sp.dockSide}
	{#key sp.dockEpoch}
		<SpawnModal
			bind:this={sp.spawnModal}
			docked={sp.dockSide}
			stacked={sp.docks.stacked}
			dockWidth={sp.docks[sp.dockSide] ?? undefined}
			prefill={sp.spawnPrefill}
			onclose={() => {
				sp.spawnPrefill = null;
				sp.dockEpoch++;
			}}
			onspawned={() => invalidateSessions()}
		/>
	{/key}
{:else if sp.showSpawn}
	<SpawnModal
		bind:this={sp.spawnModal}
		prefill={sp.spawnPrefill}
		onclose={() => {
			sp.showSpawn = false;
			sp.spawnPrefill = null;
		}}
		onspawned={() => invalidateSessions()}
	/>
{/if}

{#if sp.pendingDraftEdit}
	<EditDraftModal {sp} />
{/if}

{#if sp.archiveConfirm.pending}
	<ConfirmModal
		open
		tone="danger"
		title={sp.archiveConfirm.pending.title}
		message={sp.archiveConfirm.pending.message}
		confirmLabel={m.sessions_archive_all()}
		busy={sp.archiveConfirm.busy}
		onconfirm={sp.archiveConfirm.confirm}
		oncancel={sp.archiveConfirm.cancel}
	/>
{/if}

{#if sp.docks.stats && sp.docks[sp.docks.stats]}
	<StatsDock side={sp.docks.stats} stacked={sp.docks.stacked} width={sp.docks[sp.docks.stats] ?? ''} />
{/if}
