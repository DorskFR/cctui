<script lang="ts">
	import '$lib/styles/app.css';
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import { page } from '$app/state';
	import { auth } from '$lib/auth.svelte';
	import { settings } from '$lib/settings.svelte';
	import { ws } from '$lib/ws.svelte';
	import Header from '$lib/components/organisms/Header.svelte';
	import BottomNav from '$lib/components/organisms/BottomNav.svelte';
	import Toaster from '$lib/components/organisms/Toaster.svelte';
	import Login from '$lib/components/organisms/Login.svelte';
	import { installCodeCopy } from '$lib/codecopy';
	import { installImageLightbox } from '$lib/imagelightbox';
	import { Container } from '@dorsk/tsumikit';

	let { children } = $props();

	// The embedded review center (CCT-610) manages its own full-height layout, so
	// it renders outside the width-capped Container and without content padding.
	const isReview = $derived(page.url.pathname.startsWith('/review'));

	// One delegated listener for every code-block copy button (CCT-297 #20).
	$effect(() => {
		installCodeCopy();
	});

	// One delegated listener for every inline agent-posted image (CCT-566).
	$effect(() => {
		installImageLightbox();
	});

	const queryClient = new QueryClient({
		defaultOptions: {
			queries: { retry: 1, staleTime: 5_000, refetchOnWindowFocus: false }
		}
	});

	// Probe the `HttpOnly` auth cookie once on load to learn whether we're already
	// signed in (CCT-423) — the token isn't readable from JS.
	$effect(() => {
		void auth.init();
	});

	// Keep the websocket alive whenever we hold a valid cookie session.
	$effect(() => {
		if (auth.isAuthed) ws.connect();
		else ws.disconnect();
	});

	// Pull server-persisted user settings once a token is established (CCT-426).
	// `load()` is idempotent (runs once) and tolerates 401/offline by keeping the
	// localStorage-cached / default state, so it's safe to call on every auth flip.
	$effect(() => {
		if (auth.isAuthed) void settings.load();
	});
</script>

<QueryClientProvider client={queryClient}>
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
	<Toaster />
</QueryClientProvider>

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
