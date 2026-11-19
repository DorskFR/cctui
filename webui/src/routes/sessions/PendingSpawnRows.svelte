<script lang="ts">
	import { Link, Text } from '@dorsk/tsumikit';
	import PendingSpawnCard from '#lib/components/molecules/PendingSpawnCard.svelte';
	import { pendingSpawns } from '#lib/pendingSpawns.svelte.ts';
	import { groupFailures, useFailedDispatches } from '#lib/queries/failedDispatches.ts';
	import { m } from '#lib/paraglide/messages.js';
	import type { SessionsPage } from './sessionsPage.svelte';

	let { sp }: { sp: Pick<SessionsPage, 'items'> } = $props();

	const knownIds = $derived(sp.items.map((s) => s.id));
	const rows = $derived(pendingSpawns.visible(knownIds));
	const failedQuery = useFailedDispatches();
	const failed = $derived(groupFailures(failedQuery.data ?? []));
	const failedCount = $derived(failed.active.length + failed.older.length);
	$effect(() => {
		pendingSpawns.adopt(sp.items);
		pendingSpawns.settle(knownIds);
	});
</script>

{#if rows.length > 0 || failedCount > 0}
	<div class="rows" data-journey="section" data-journey-key="pending">
		{#each rows as p (p.commandId)}
			<PendingSpawnCard spawn={p} ondismiss={(id) => pendingSpawns.dismiss(id)} />
		{/each}
		{#if failedCount > 0}
			<Link href="/dispatch-failures" data-journey="failed-dispatches">
				<Text size="xs">{m.dispatch_failed_link({ count: failedCount })}</Text>
			</Link>
		{/if}
	</div>
{/if}

<style>
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
</style>
