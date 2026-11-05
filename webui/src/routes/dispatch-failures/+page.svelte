<script lang="ts">
	import { Badge, Card, Spinner, Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import FailedDispatchCard from '$lib/components/molecules/FailedDispatchCard.svelte';
	import { errMessage } from '$lib/api';
	import { useFailedDispatchHistory, useRedispatch } from '$lib/queries/failedDispatches';
	import { m } from '$lib/paraglide/messages';
	import { groupByCause } from './causes';

	const history = useFailedDispatchHistory();
	const redispatch = useRedispatch();
	const rows = $derived(history.data ?? []);
	const resolved = $derived(rows.filter((d) => d.superseded_by));
	const causes = $derived(groupByCause(rows.filter((d) => !d.superseded_by)));
</script>

<div class="page" data-journey="dispatch-failures">
	<PageHead title={m.dispatch_failed_history()} subtitle={m.dispatch_failed_history_subtitle()} />

	{#if history.isLoading}
		<div class="msg"><Spinner /></div>
	{:else if history.error}
		<div class="msg"><Text tone="danger">{errMessage(history.error)}</Text></div>
	{:else if rows.length === 0}
		<div class="msg"><Text tone="faint">{m.dispatch_failed_history_empty()}</Text></div>
	{:else}
		{#each causes as c (c.key)}
			<Card padding="md" data-cause={c.key}>
				<section class="cause">
					<header class="cause-head">
						<Badge tone="danger" size="sm">{c.reason}</Badge>
						<Text size="sm" tone="muted" numeric>{m.list_failed_cause_count({ count: c.items.length })}</Text>
					</header>
					{#if c.explanation}
						<Text size="sm" tone="muted">{c.explanation}</Text>
					{/if}
					<ul class="rows">
						{#each c.items as d (d.session_id)}
							<li><FailedDispatchCard dispatch={d} onrerun={redispatch} showCause={false} /></li>
						{/each}
					</ul>
				</section>
			</Card>
		{/each}
		{#if resolved.length > 0}
			<Card padding="md" data-cause="resolved">
				<section class="cause">
					<header class="cause-head">
						<Badge tone="ok" size="sm">{m.dispatch_failed_resolved()}</Badge>
						<Text size="sm" tone="muted" numeric>{m.list_failed_cause_count({ count: resolved.length })}</Text>
					</header>
					<ul class="rows">
						{#each resolved as d (d.session_id)}
							<li><FailedDispatchCard dispatch={d} onrerun={redispatch} /></li>
						{/each}
					</ul>
				</section>
			</Card>
		{/if}
	{/if}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		padding: var(--sp-4);
		max-width: var(--content-wide);
		width: 100%;
		margin: 0 auto;
	}
	.cause {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.cause-head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.rows {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.rows li {
		padding-block: var(--sp-2);
		border-top: 1px solid var(--divider);
	}
	.msg {
		display: grid;
		place-items: center;
		padding: var(--sp-6);
	}
</style>
