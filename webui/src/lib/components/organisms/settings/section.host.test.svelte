<script lang="ts">
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import type { Component } from 'svelte';

	let { section: Section }: { section: Component<Record<string, never>> } = $props();

	const client = new QueryClient({
		defaultOptions: { queries: { retry: false }, mutations: { retry: false } }
	});
</script>

<QueryClientProvider {client}>
	<Section />
</QueryClientProvider>
