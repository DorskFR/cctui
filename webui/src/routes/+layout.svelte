<script lang="ts">
	import '$lib/styles/app.css';
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import { auth } from '$lib/auth.svelte';
	import { ws } from '$lib/ws.svelte';
	import Header from '$lib/components/Header.svelte';
	import BottomNav from '$lib/components/BottomNav.svelte';
	import Toaster from '$lib/components/Toaster.svelte';
	import Login from '$lib/components/Login.svelte';
	import { installCodeCopy } from '$lib/codecopy';

	let { children } = $props();

	// One delegated listener for every code-block copy button (CCT-297 #20).
	$effect(() => {
		installCodeCopy();
	});

	const queryClient = new QueryClient({
		defaultOptions: {
			queries: { retry: 1, staleTime: 5_000, refetchOnWindowFocus: false }
		}
	});

	// Keep the websocket alive whenever we hold a token.
	$effect(() => {
		if (auth.isAuthed) ws.connect();
		else ws.disconnect();
	});
</script>

<QueryClientProvider client={queryClient}>
	{#if auth.isAuthed}
		<div class="app">
			<Header />
			<main class="content">
				<div class="container">
					{@render children?.()}
				</div>
			</main>
			<BottomNav />
		</div>
	{:else}
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
</style>
