<script lang="ts">
	import '$lib/styles/app.css';
	import { QueryClient } from '@tanstack/svelte-query';
	import { PersistQueryClientProvider } from '@tanstack/svelte-query-persist-client';
	import { createAsyncStoragePersister } from '@tanstack/query-async-storage-persister';
	import { get as idbGet, set as idbSet, del as idbDel } from 'idb-keyval';
	import { qk } from '$lib/queries';
	import { attachmentStore } from '$lib/attachmentStore';
	import type { SessionListResponse } from '@bindings/SessionListResponse';
	import { page } from '$app/state';
	import { auth } from '$lib/auth.svelte';
	import { settings, sessionListWidthSize } from '$lib/settings.svelte';
	import { locale } from '$lib/locale.svelte';
	import { ws } from '$lib/ws.svelte';
	import { goto } from '$app/navigation';
	import { sessionFailureToast } from '$lib/sessionFailureToast';
	import Header from '$lib/components/organisms/Header.svelte';
	import BottomNav from '$lib/components/organisms/BottomNav.svelte';
	import Login from '$lib/components/organisms/Login.svelte';
	import { installCodeCopy } from '$lib/codecopy';
	import { installImageLightbox } from '$lib/imagelightbox';
	import { Container, Toaster } from '@dorsk/tsumikit';
	import { dockLayout } from '$lib/spawnDock.svelte';

	let { children } = $props();

	// The embedded review center manages its own full-height layout, so
	// it renders outside the width-capped Container and without content padding.
	const isReview = $derived(page.url.pathname.startsWith('/github'));

	// Every route renders in the --content-wide column; only the session list's
	// width is user-settable (Settings › Session list), and the cap only bites
	// above it, so a phone is unaffected whatever is picked.
	const contentSize = $derived(
		(page.url.pathname.startsWith('/sessions')
			? sessionListWidthSize(settings.sessionListWidth)
			: undefined) ?? 'var(--content-wide)'
	);
	const topNav = $derived(settings.nav === 'top');

	// Published on the document element so --bottom-chrome (app.css) resolves
	// for fixed panels wherever they sit in the tree.
	$effect(() => {
		document.documentElement.dataset.nav = topNav ? 'top' : 'bottom';
	});

	// Docked panels (Settings › New session / Stats panel): the Sessions screen
	// pins the spawn form and/or the stats panel to an edge, so the content
	// reserves that edge for them. The panels themselves are rendered by the
	// Sessions page (position: fixed); this padding is what keeps the centered
	// list out from under them.
	const docks = $derived(page.url.pathname.startsWith('/sessions') ? dockLayout() : null);

	// One delegated listener for every code-block copy button.
	$effect(() => {
		installCodeCopy();
	});

	// One delegated listener for every inline agent-posted image.
	$effect(() => {
		installImageLightbox();
	});

	const queryClient = new QueryClient({
		defaultOptions: {
			queries: { retry: 1, staleTime: 5_000, refetchOnWindowFocus: false }
		}
	});

	// Persist the heavyweight caches to IndexedDB so a reload paints from disk
	// and revalidates as a delta/304 instead of re-downloading everything.
	const PERSISTED = new Set(['sessions', 'conversation', 'labels']);
	const persister = createAsyncStoragePersister({
		storage: {
			getItem: (k: string) => idbGet<string>(k).then((v) => v ?? null),
			setItem: (k: string, v: string) => idbSet(k, v),
			removeItem: (k: string) => idbDel(k)
		},
		key: 'cctui-query-cache'
	});
	const persistOptions = {
		persister,
		maxAge: 24 * 60 * 60 * 1000,
		dehydrateOptions: {
			shouldDehydrateQuery: (q: { queryKey: readonly unknown[]; state: { status: string } }) =>
				q.state.status === 'success' && PERSISTED.has(q.queryKey[0] as string)
		}
	};

	// Restore-time reconciliation: drop persisted conversations, and IndexedDB
	// attachment records, whose session is archived or no longer in the
	// restored list (plus, for attachments, anything long untouched).
	function purgeStaleConversations() {
		const live = queryClient.getQueryData<SessionListResponse>(qk.sessions(false));
		void attachmentStore.sweep(live?.sessions ?? null);
		if (!live) return;
		const keep = new Set(live.sessions.filter((s) => s.status !== 'archived').map((s) => s.id));
		for (const q of queryClient.getQueryCache().findAll({ queryKey: ['conversation'] })) {
			const sid = q.queryKey[1] as string;
			if (!keep.has(sid)) queryClient.removeQueries({ queryKey: q.queryKey, exact: true });
		}
	}

	// Probe the `HttpOnly` auth cookie once on load to learn whether we're already
	// signed in — the token isn't readable from JS.
	$effect(() => {
		void auth.init();
	});

	// Keep the websocket alive whenever we hold a valid cookie session.
	$effect(() => {
		if (auth.isAuthed) ws.connect();
		else ws.disconnect();
	});

	$effect(() => ws.onSessionEnded((ev) => sessionFailureToast(ev, (href) => void goto(href))));

	// Pull server-persisted user settings once a token is established.
	// `load()` is idempotent (runs once) and tolerates 401/offline by keeping the
	// localStorage-cached / default state, so it's safe to call on every auth flip.
	$effect(() => {
		if (auth.isAuthed) void settings.load();
	});
</script>

<PersistQueryClientProvider
	client={queryClient}
	{persistOptions}
	onSuccess={purgeStaleConversations}
>
	<!-- Remount the tree on a language flip so labels captured in component-init
	     `const`s (not just reactive template reads) re-localize live. -->
	{#key locale.current}
		{#if auth.isAuthed}
			<div class="app">
				<Header />
				<main
					class="content"
					class:review={isReview}
					class:dock-left={!!docks?.left}
					class:dock-right={!!docks?.right}
					style:--dock-left-w={docks?.left ?? undefined}
					style:--dock-right-w={docks?.right ?? undefined}
				>
					{#if isReview}
						{@render children?.()}
					{:else}
						<Container size={contentSize}>
							{@render children?.()}
						</Container>
					{/if}
				</main>
				<BottomNav />
			</div>
		{:else if !auth.checking}
			<Login />
		{/if}
	{/key}
	<div class="toaster-host" data-toast-pos={settings.toastPosition}><Toaster /></div>
</PersistQueryClientProvider>

<style>
	.app {
		min-height: 100dvh;
		display: flex;
		flex-direction: column;
	}
	.content {
		flex: 1;
		/* clear the fixed header and bottom nav (+ safe areas) */
		padding-top: calc(var(--header-h) + var(--safe-top) + var(--sp-3));
		padding-bottom: calc(var(--bottom-chrome) + var(--sp-4));
	}
	/* Reserve the docked panels' edges (Sessions screen only). */
	.content.dock-left {
		padding-left: var(--dock-left-w);
	}
	.content.dock-right {
		padding-right: var(--dock-right-w);
	}
	/* The review center fills the viewport between header and nav and scrolls
	   internally, so drop the content padding and pin a definite height. */
	.content.review {
		display: flex;
		flex-direction: column;
		height: 100dvh;
		padding-top: calc(var(--header-h) + var(--safe-top));
		padding-bottom: var(--bottom-chrome);
	}
</style>
