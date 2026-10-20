<script lang="ts">
	import PendingSpawnCard from '$lib/components/molecules/PendingSpawnCard.svelte';
	import { pendingSpawns } from '$lib/pendingSpawns.svelte';
	import type { SessionsPage } from './sessionsPage.svelte';

	let { sp }: { sp: SessionsPage } = $props();

	const knownIds = $derived(sp.items.map((s) => s.id));
	const rows = $derived(pendingSpawns.visible(knownIds));
	$effect(() => {
		pendingSpawns.adopt(sp.items);
		pendingSpawns.settle(knownIds);
	});
</script>

{#if rows.length > 0}
	<div class="rows" data-journey="section" data-journey-key="pending">
		{#each rows as p (p.commandId)}
			<PendingSpawnCard spawn={p} ondismiss={(id) => pendingSpawns.dismiss(id)} />
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
