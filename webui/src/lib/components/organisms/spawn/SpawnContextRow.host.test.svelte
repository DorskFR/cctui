<script lang="ts">
	import { untrack } from 'svelte';
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import SpawnContextRow from './SpawnContextRow.svelte';

	let {
		client,
		onready
	}: {
		client: QueryClient;
		onready: (form: { context_items: string[]; context_auto: boolean; pins: string[] }) => void;
	} = $props();

	let form = $state({
		adapter_id: 'claude-code',
		context_items: [] as string[],
		context_auto: true,
		pins: [] as string[]
	});
	// Mirrors `applySpec`: the profile's form is a spread of the whole form.
	const effectiveForm = $derived({ ...form, adapter_id: 'codex' });
	untrack(() => onready(form));
</script>

<QueryClientProvider {client}>
	<SpawnContextRow
		bind:items={form.context_items}
		bind:auto={form.context_auto}
		bind:pins={form.pins}
		machineId="m-1"
		workingDir="/repo"
		labels={[]}
		adapter={effectiveForm.adapter_id}
		pinnedIds={['id-pin']}
	/>
</QueryClientProvider>
