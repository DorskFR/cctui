<script lang="ts">
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import type { ComponentProps } from 'svelte';
	import Conversation from './Conversation.svelte';

	let { props }: { props: ComponentProps<typeof Conversation> } = $props();

	const client = new QueryClient({
		defaultOptions: { queries: { retry: false }, mutations: { retry: false } }
	});
</script>

<QueryClientProvider {client}>
	<Conversation {...props} />
</QueryClientProvider>
