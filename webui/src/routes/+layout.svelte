<script lang="ts">
	import '$lib/styles/app.css';
	import { QueryClient } from '@tanstack/svelte-query';
	import { PersistQueryClientProvider } from '@tanstack/svelte-query-persist-client';
	import { createAsyncStoragePersister } from '@tanstack/query-async-storage-persister';
	import { get as idbGet, set as idbSet, del as idbDel } from 'idb-keyval';
	import { qk } from '$lib/queries';
	import type { SessionListResponse } from '@bindings/SessionListResponse';
	import { page } from '$app/state';
	import { auth } from '$lib/auth.svelte';
	import { settings } from '$lib/settings.svelte';
	import { locale } from '$lib/locale.svelte';
	import { ws } from '$lib/ws.svelte';
	import Header from '$lib/components/organisms/Header.svelte';
	import BottomNav from '$lib/components/organisms/BottomNav.svelte';
	import Toaster from '$lib/components/organisms/Toaster.svelte';
	import Login from '$lib/components/organisms/Login.svelte';
	import { installCodeCopy } from '$lib/codecopy';
	import { installImageLightbox } from '$lib/imagelightbox';
	import { Container } from '@dorsk/tsumikit';

	let { children } = $props();

	// The embedded review center manages its own full-height layout, so
	// it renders outside the width-capped Container and without content padding.
	const isReview = $derived(page.url.pathname.startsWith('/github'));

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

	// Restore-time reconciliation: drop persisted conversations whose session
	// is archived or no longer in the restored list.
	function purgeStaleConversations() {
		const live = queryClient.getQueryData<SessionListResponse>(qk.sessions(false));
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
				<main class="content" class:review={isReview}>
					{#if isReview}
						{@render children?.()}
					{:else}
						<Container>
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
	<Toaster />
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
		padding-bottom: calc(var(--nav-h) + var(--safe-bottom) + var(--sp-4));
	}
	/* The review center fills the viewport between header and nav and scrolls
	   internally, so drop the content padding and pin a definite height. */
	.content.review {
		display: flex;
		flex-direction: column;
		height: 100dvh;
		padding-top: calc(var(--header-h) + var(--safe-top));
		padding-bottom: calc(var(--nav-h) + var(--safe-bottom));
	}
</style>
