<script lang="ts">
	import { Button, Icon, Link, Text } from '@dorsk/tsumikit';
	import PendingSpawnCard from '$lib/components/molecules/PendingSpawnCard.svelte';
	import { errMessage } from '$lib/api';
	import { confirmDialog } from '$lib/confirmDialog.svelte';
	import { pendingSpawns } from '$lib/pendingSpawns.svelte';
	import {
		groupFailures,
		useDismissFailedDispatch,
		useFailedDispatches
	} from '$lib/queries/failedDispatches';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import { summarizeFailures } from '../dispatch-failures/causes';
	import type { SessionsPage } from './sessionsPage.svelte';

	let { sp }: { sp: SessionsPage } = $props();

	const knownIds = $derived(sp.items.map((s) => s.id));
	const rows = $derived(pendingSpawns.visible(knownIds));
	const failedQuery = useFailedDispatches();
	const failed = $derived(groupFailures(failedQuery.data ?? []));
	const summary = $derived(summarizeFailures([...failed.active, ...failed.older]));
	const dismiss = useDismissFailedDispatch();
	let clearing = $state(false);
	$effect(() => {
		pendingSpawns.adopt(sp.items);
		pendingSpawns.settle(knownIds);
	});

	async function dismissAll() {
		const ids = summary?.ids ?? [];
		if (ids.length === 0) return;
		const ok = await confirmDialog({
			title: m.list_failed_dismiss_title({ count: ids.length }),
			message: m.list_failed_dismiss_message(),
			confirmLabel: m.dispatch_failed_dismiss_all(),
			tone: 'warn'
		});
		if (!ok) return;
		clearing = true;
		try {
			const failures = (await Promise.allSettled(ids.map((id) => dismiss(id)))).filter(
				(r): r is PromiseRejectedResult => r.status === 'rejected'
			);
			if (failures.length > 0) toasts.error(errMessage(failures[0].reason));
		} finally {
			clearing = false;
		}
	}
</script>

{#if rows.length > 0 || summary}
	<div class="rows" data-journey="section" data-journey-key="pending">
		{#each rows as p (p.commandId)}
			<PendingSpawnCard spawn={p} ondismiss={(id) => pendingSpawns.dismiss(id)} />
		{/each}
		{#if summary}
			<div class="fail-summary" role="status" data-journey="failed-dispatch-summary">
				<span class="mark"><Icon name="warning" size={16} /></span>
				<Text weight="semibold" numeric>{m.list_failed_summary({ count: summary.count })}</Text>
				<span class="cause">
					<Text size="sm" tone="muted" truncate>
						{summary.cause ?? m.list_failed_mixed({ count: summary.causes })}
					</Text>
				</span>
				<span class="spacer"></span>
				<Button as="a" href="/dispatch-failures" size="sm" data-journey="older-failures">
					{m.list_failed_review()}
				</Button>
				<Button size="sm" variant="ghost" disabled={clearing} onclick={dismissAll}>
					{m.dispatch_failed_dismiss_all()}
				</Button>
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
	.fail-summary {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-1) var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-radius: var(--r-md);
		border-left: var(--status-rail-w) solid var(--status-attention);
		background: var(--bg-elevated);
	}
	.mark {
		display: inline-flex;
		color: var(--status-attention);
	}
	.cause {
		display: inline-flex;
		min-width: 0;
		flex: 0 1 auto;
	}
	.spacer {
		flex: 1;
	}
</style>
