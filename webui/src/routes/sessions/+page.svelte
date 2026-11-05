<script lang="ts">
	import { untrack, onMount } from 'svelte';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import {
		useSessions,
		useSessionActions,
		useInvalidateSessions,
		useLabels,
		useMachineResources,
		useMe,
		endpoints,
		qk
	} from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { page } from '$app/state';
	import { pushState, replaceState } from '$app/navigation';
	import { toasts } from '$lib/toast.svelte';
	import { ws } from '$lib/ws.svelte';
	import ConversationDrawer from '$lib/components/organisms/ConversationDrawer.svelte';
	import ConversationDock from '$lib/components/organisms/ConversationDock.svelte';
	import SpawnModal from '$lib/components/organisms/SpawnModal.svelte';
	import { dockLayout } from '$lib/spawnDock.svelte';
	import StatsDock from '$lib/components/organisms/statsdock/StatsDock.svelte';
	import SessionControls from '$lib/components/organisms/SessionControls.svelte';
	import { Callout, ConfirmModal, Icon, Toggle } from '@dorsk/tsumikit';
	import { visibleSessions } from '$lib/hiddenOwners';
	import SessionSections from './SessionSections.svelte';
	import SessionTiles from './SessionTiles.svelte';
	import TilesOverflowChip from './TilesOverflowChip.svelte';
	import SessionsBulkBar from './SessionsBulkBar.svelte';
	import EditDraftModal from './EditDraftModal.svelte';
	import {
		drafts,
		clearSpawnSlot,
		currentSpawnSlot,
		readSpawnSlot,
		LAST_DOCKED_SESSION
	} from '$lib/drafts';
	import { notify } from '$lib/notify.svelte';
	import { displayNameOf } from '$lib/sessionName';
	import { isViewMode, parseViewMode } from '$lib/sessionsView.svelte';
	import { TILES_BOOT_KEY } from './tilesBoot';
	import { settings } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';
	import {
		sessionIdFromLocation,
		sessionHrefFor,
		sessionNavState,
		toGroupDimension,
		urlSyncFor,
		withoutViewParam
	} from './sessions.logic';
	import { SessionsPage } from './sessionsPage.svelte';

	// Live buckets always show non-archived sessions; the archive is a separate
	// paginated section below.
	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');
	let showHidden = $state(false);
	const sessions = useSessions(
		() => false,
		() => true,
		() => isAdmin && showHidden
	);
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
		queryClient: qc,
		invalidateSessions,
		refetchLive: () => qc.invalidateQueries({ queryKey: qk.sessions(false) }),
		dockLayout,
		clearUrlSession: () => replaceUrlSession(null),
		confirm: (message) => confirm(message),
		spawnSlot: {
			currentKey: currentSpawnSlot,
			read: readSpawnSlot,
			write: (key, payload) => drafts.set(key, JSON.stringify(payload)),
			clear: clearSpawnSlot
		},
		sessions: () => visibleSessions(sessions.data?.sessions ?? [], isAdmin && showHidden),
		sessionsLoading: () => sessions.isLoading,
		allSessions: () => allSessions.data?.sessions ?? [],
		labels: () => labelsQuery.data?.labels,
		renderedOrder: renderedIds,
		showHidden: () => isAdmin && showHidden
	});

	// Shallow routing never advances `page.url`, and popstate restores the
	// `page.url` saved with the entry, not the entry's address: only the address
	// bar is current. `page.url`/`page.state` are read to re-run on navigation.
	const liveUrl = (): URL => {
		void page.url;
		void page.state;
		return new URL(location.href);
	};

	// Escape hatch: /sessions?view=list|grid|tiles sets (and persists) the mode,
	// so a stored choice that misbehaves can always be overridden by URL.
	$effect(() => {
		const want = liveUrl().searchParams.get('view');
		if (!want || !(isViewMode(want) || want === 'card')) return;
		untrack(() => {
			sp.viewMode = parseViewMode(want);
			const href = withoutViewParam(location.href);
			if (href) replaceState(href, page.state);
		});
	});

	// A tiles render that takes the tab down never clears this breadcrumb, so the
	// next load finds it and starts in the list instead of crashing again. Read at
	// init: an effect would race the effect below that sets it.
	const tilesCrashed = sessionStorage.getItem(TILES_BOOT_KEY) === '1';
	if (tilesCrashed) {
		sessionStorage.removeItem(TILES_BOOT_KEY);
		if (sp.tiles) sp.viewMode = 'list';
	}
	onMount(() => {
		if (tilesCrashed) toasts.error(m.tiles_failed());
	});
	$effect(() => {
		if (!sp.tiles) return;
		sessionStorage.setItem(TILES_BOOT_KEY, '1');
		const done = requestAnimationFrame(() => sessionStorage.removeItem(TILES_BOOT_KEY));
		return () => {
			cancelAnimationFrame(done);
			sessionStorage.removeItem(TILES_BOOT_KEY);
		};
	});

	// The full (incl. archived) list, only fetched while a pinned parent may
	// have archived subagents to splice back under it.
	const allSessions = useSessions(
		() => true,
		() => sp.pinnedIds.size > 0
	);

	// Machine liveness for the group headers; falls back to "unknown" (no dot)
	// when the machines endpoint is not readable.
	const machines = useMachineResources(() => sp.groupBy === 'machine');
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
	// Seeded from the URL on mount so a deep-link load doesn't double-push.
	// `mounted` is reactive so the drawer→URL effect re-runs once seeded.
	let mounted = $state(false);
	// Id opened without a navigation of its own (the restored dock).
	let quietOpen: string | null = null;
	let tilesOverflow = $state<SessionListItem[]>([]);
	// Not `page.params`/`page.url`: shallow routing leaves both on the last full
	// navigation, so they can name a session the address bar no longer shows.
	const sessionIdFromUrl = (): string | null => {
		const url = liveUrl();
		return sessionIdFromLocation(url.pathname, url.searchParams);
	};
	onMount(() => {
		lastUrlId = sessionIdFromUrl();
		mounted = true;
	});

	function replaceUrlSession(id: string | null) {
		const href = sessionHrefFor(location.href, id);
		if (href !== null) replaceState(href, {});
	}

	function syncUrlSession(id: string | null, quiet: boolean) {
		const urlId = sessionIdFromLocation(location.pathname, new URLSearchParams(location.search));
		const { fromList } = sessionNavState(untrack(() => page.state));
		const how = urlSyncFor({ to: id, urlId, fromList: !!fromList, quiet });
		if (how === 'back') return history.back();
		const href = how && sessionHrefFor(location.href, id);
		if (!href) return;
		if (how === 'push') pushState(href, { fromList: true } as App.PageState);
		else replaceState(href, (id && fromList ? { fromList } : {}) as App.PageState);
	}

	// URL → drawer: react to the `session` param (initial load, back/forward,
	// pasted link). Only act when it differs from what's already open. The
	// `openSession` read is untracked: if this effect depended on it,
	// any `openSession = …` (card click, notification) would re-run it *before*
	// the drawer→URL effect below writes the id — the still-empty URL
	// param then hit the `openSession = null` branch and closed the drawer in
	// the same flush, so conversations never opened. Depending only on the URL
	// keeps this effect to its job: URL changes drive the drawer, not vice versa.
	$effect(() => {
		const id = sessionIdFromUrl();
		if (id === untrack(() => sp.openSession?.id ?? null)) return;
		if (!id) {
			sp.openSession = null;
			return;
		}
		const spawned = sessionNavState(untrack(() => page.state)).spawned === id;
		void sp.openById(id, { spawned });
	});

	// drawer → URL: reflect the open session into the address bar so it's always
	// a shareable link. Skip while we're resolving a URL-driven open (no echo).
	$effect(() => {
		const id = sp.openSession?.id ?? null;
		if (sp.urlResolving) return;
		if (!mounted) return;
		if (id === lastUrlId) return;
		syncUrlSession(id, id !== null && id === quietOpen);
		quietOpen = null;
		lastUrlId = id;
	});

	// Docked conversation: remember what it shows, and on arrival with no
	// session in the URL reopen the last one, if it is still in the live list
	// (a session archived or deleted since simply leaves the column empty).
	let dockRestored = false;
	$effect(() => {
		if (dockRestored || !mounted || !sp.docks.conversation || sessions.isLoading) return;
		dockRestored = true;
		if (untrack(() => sp.openSession) || sessionIdFromUrl()) return;
		const last = drafts.get(LAST_DOCKED_SESSION);
		const row = last ? untrack(() => sp.items.find((s) => s.id === last)) : undefined;
		if (!row) return;
		quietOpen = row.id;
		sp.openSession = row;
	});
	$effect(() => {
		const id = sp.openSession?.id;
		if (id && sp.docks.conversation) drafts.set(LAST_DOCKED_SESSION, id);
	});
	const closeDocked = () => {
		drafts.clear(LAST_DOCKED_SESSION);
		sp.openSession = null;
	};

	// live status changes from the websocket → refetch the list
	$effect(() => {
		void ws.changeTick;
		invalidateSessions();
	});

	$effect(() => {
		notify.setSessionTitle(sp.liveOpen ? displayNameOf(sp.liveOpen) : null);
		return () => notify.setSessionTitle(null);
	});

	// Tell the notifier which drawer is open so it won't notify for it.
	$effect(() => {
		const id = sp.openSession?.id;
		if (!id) return;
		return notify.holdOpen(id);
	});

	// Unread tracking: mark every on-screen session's messages seen server-side,
	// then refetch so its badge drops to zero. `changeTick` re-runs it as new
	// messages stream in while a pane stays open, keeping those sessions at zero
	// instead of re-accumulating unread.
	$effect(() => {
		void ws.changeTick;
		const ids = [...notify.openSessionIds];
		if (ids.length === 0) return;
		void Promise.allSettled(ids.map((id) => actions.markSeen(id))).then(() =>
			invalidateSessions()
		);
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


{#snippet trailingControls()}
	{#if isAdmin}
		<Toggle
			pressed={showHidden}
			title={m.hidden_owners_show_hidden_title()}
			data-journey="show-hidden"
			onclick={() => (showHidden = !showHidden)}
		>
			<Icon name={showHidden ? 'eye' : 'eye-off'} size={14} />
			{m.hidden_owners_show_hidden()}
		</Toggle>
	{/if}
	{#if sp.tiles && tilesOverflow.length}
		<TilesOverflowChip items={tilesOverflow} onOpen={(s) => (sp.openSession = s)} />
	{/if}
{/snippet}

<SessionControls
	bind:rawQuery={sp.rawQuery}
	searchSchema={sp.searchSchema}
	bind:sections={sp.sections}
	labels={sp.allLabels}
	bind:labelFilter={sp.labelFilter}
	bind:view={sp.viewMode}
	tiles={!sp.mobile}
	sticky={!sp.tiles}
	flush={sp.tiles}
	colorBy={sp.colorBy}
	groupBy={sp.groupBy}
	onColorBy={sp.setColorBy}
	onGroupBy={(v) => settings.setSessionList({ groupBy: toGroupDimension(v) })}
	selecting={sp.list.selecting}
	searching={sp.searching}
	onStartSelect={() => (sp.list.selecting = true)}
	onCancelSelect={sp.list.exitSelect}
	onNew={sp.dockSide ? undefined : () => (sp.showSpawn = true)}
	trailing={isAdmin || (sp.tiles && tilesOverflow.length) ? trailingControls : undefined}
	onUpdateLabel={sp.updateLabel}
	onDeleteLabel={sp.deleteLabel}
	scopeMachine={sp.openSession ? (sp.openSession.machine_name ?? sp.openSession.machine_id) : null}
	scopeDir={sp.openSession?.working_dir || null}
/>

{#if sp.list.selecting}
	<SessionsBulkBar {sp} />
{/if}

{#if sp.tiles}
	<svelte:boundary onerror={(e) => sp.abandonTiles(e)}>
		<SessionTiles
			sessions={sp.tileSessions}
			onNavigate={(sid) => void sp.navigateToForked(sid)}
			onFollowup={sp.followUp}
			onoverflow={(items) => (tilesOverflow = items)}
		/>
		{#snippet failed()}
			<Callout tone="danger">{m.tiles_failed()}</Callout>
		{/snippet}
	</svelte:boundary>
{:else}
	<SessionSections {sp} {pending} {machineLiveness} />
{/if}

{#if sp.docks.conversation}
	<ConversationDock
		side={sp.docks.conversation}
		width={sp.docks[sp.docks.conversation] ?? ''}
		session={sp.liveOpen}
		onclose={closeDocked}
		highlight={sp.searchTerms}
		focusSeq={sp.focusSeq}
		onNewFromScript={sp.newFromScript}
		onFollowup={sp.followUp}
		onNavigate={(sid) => void sp.navigateToForked(sid)}
	/>
{:else if sp.liveOpen}
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
