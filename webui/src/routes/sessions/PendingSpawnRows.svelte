<script lang="ts">
	import { Button, Link, Text } from '@dorsk/tsumikit';
	import FailedDispatchCard from '$lib/components/molecules/FailedDispatchCard.svelte';
	import PendingSpawnCard from '$lib/components/molecules/PendingSpawnCard.svelte';
	import { errMessage } from '$lib/api';
	import { pendingSpawns } from '$lib/pendingSpawns.svelte';
	import {
		groupFailures,
		useDismissAllFailedDispatches,
		useDismissFailedDispatch,
		useFailedDispatches,
		useRedispatch
	} from '$lib/queries/failedDispatches';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import type { SessionsPage } from './sessionsPage.svelte';

	let { sp }: { sp: SessionsPage } = $props();

	const knownIds = $derived(sp.items.map((s) => s.id));
	const rows = $derived(pendingSpawns.visible(knownIds));
	const failedQuery = useFailedDispatches();
	const failed = $derived(groupFailures(failedQuery.data ?? []));
	const shown = $derived(failed.active.length + failed.resolved.length);
	const redispatch = useRedispatch();
	const dismiss = useDismissFailedDispatch();
	const dismissAll = useDismissAllFailedDispatches();
	let clearing = $state(false);
	$effect(() => {
		pendingSpawns.adopt(sp.items);
		pendingSpawns.settle(knownIds);
	});

	async function clearAll() {
		clearing = true;
		try {
			await dismissAll();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			clearing = false;
		}
	}
</script>

{#if rows.length > 0 || shown > 0 || failed.older.length > 0}
	<div class="rows" data-journey="section" data-journey-key="pending">
		{#each rows as p (p.commandId)}
			<PendingSpawnCard spawn={p} ondismiss={(id) => pendingSpawns.dismiss(id)} />
		{/each}
		{#each [...failed.active, ...failed.resolved] as d (d.session_id)}
			<FailedDispatchCard dispatch={d} onrerun={redispatch} ondismiss={dismiss} />
		{/each}
		{#if shown > 0 || failed.older.length > 0}
			<div class="foot">
				{#if failed.older.length > 0}
					<Link href="/dispatch-failures" data-journey="older-failures">
						<Text size="xs">{m.dispatch_failed_older({ count: failed.older.length })}</Text>
					</Link>
				{/if}
				<span class="spacer"></span>
				{#if shown > 1 || failed.older.length > 0}
					<Button size="sm" variant="link" disabled={clearing} onclick={clearAll}>
						{m.dispatch_failed_dismiss_all()}
					</Button>
				{/if}
			</div>
		{/if}
	</div>
{/if}

<style>
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.foot {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.spacer {
		flex: 1;
	}
</style>
