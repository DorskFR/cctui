<script lang="ts">
	import FailedDispatchCard from '$lib/components/molecules/FailedDispatchCard.svelte';
	import PendingSpawnCard from '$lib/components/molecules/PendingSpawnCard.svelte';
	import { pendingSpawns } from '$lib/pendingSpawns.svelte';
	import { useFailedDispatches, useRedispatch } from '$lib/queries/failedDispatches';
	import type { SessionsPage } from './sessionsPage.svelte';

	let { sp }: { sp: SessionsPage } = $props();

	const knownIds = $derived(sp.items.map((s) => s.id));
	const rows = $derived(pendingSpawns.visible(knownIds));
	const failedQuery = useFailedDispatches();
	const failed = $derived(failedQuery.data ?? []);
	const redispatch = useRedispatch();
	$effect(() => {
		pendingSpawns.adopt(sp.items);
		pendingSpawns.settle(knownIds);
	});
</script>

{#if rows.length > 0 || failed.length > 0}
	<div class="rows" data-journey="section" data-journey-key="pending">
		{#each rows as p (p.commandId)}
			<PendingSpawnCard spawn={p} ondismiss={(id) => pendingSpawns.dismiss(id)} />
		{/each}
		{#each failed as d (d.session_id)}
			<FailedDispatchCard dispatch={d} onrerun={redispatch} />
		{/each}
	</div>
{/if}

<style>
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
</style>
