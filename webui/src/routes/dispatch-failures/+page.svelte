<script lang="ts">
	import { Spinner, Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import FailedDispatchCard from '$lib/components/molecules/FailedDispatchCard.svelte';
	import { errMessage } from '$lib/api';
	import { useFailedDispatchHistory, useRedispatch } from '$lib/queries/failedDispatches';
	import { m } from '$lib/paraglide/messages';

	const history = useFailedDispatchHistory();
	const redispatch = useRedispatch();
	const rows = $derived(history.data ?? []);
</script>

<div class="page" data-journey="dispatch-failures">
	<PageHead title={m.dispatch_failed_history()} />
	<Text size="sm" tone="faint">{m.dispatch_failed_history_subtitle()}</Text>

	{#if history.isLoading}
		<div class="msg"><Spinner /></div>
	{:else if history.error}
		<div class="msg"><Text tone="danger">{errMessage(history.error)}</Text></div>
	{:else if rows.length === 0}
		<div class="msg"><Text tone="faint">{m.dispatch_failed_history_empty()}</Text></div>
	{:else}
		<div class="cards">
			{#each rows as d (d.session_id)}
				<FailedDispatchCard dispatch={d} onrerun={redispatch} />
			{/each}
		</div>
	{/if}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		padding: var(--sp-4);
		max-width: 60rem;
		width: 100%;
		margin: 0 auto;
	}
	.cards {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.msg {
		display: grid;
		place-items: center;
		padding: var(--sp-6);
	}
</style>
