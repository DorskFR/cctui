<script lang="ts">
	import { tick } from 'svelte';
	import { page } from '$app/state';
	import { Button, Spinner, Text } from '@dorsk/tsumikit';
	import PageHead from '#lib/components/molecules/PageHead.svelte';
	import FailedDispatchCard from '#lib/components/molecules/FailedDispatchCard.svelte';
	import { errMessage } from '#lib/api.ts';
	import {
		useDismissAllFailedDispatches,
		useDismissFailedDispatch,
		useFailedDispatchHistory,
		useRedispatch
	} from '#lib/queries/failedDispatches.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	const history = useFailedDispatchHistory();
	const redispatch = useRedispatch();
	const dismiss = useDismissFailedDispatch();
	const dismissAll = useDismissAllFailedDispatches();
	const rows = $derived(history.data ?? []);
	const undismissed = $derived(rows.filter((d) => !d.dismissed).length);
	const focused = $derived(page.url.searchParams.get('record'));
	let clearing = $state(false);

	$effect(() => {
		const id = focused;
		if (!id || !rows.some((d) => d.session_id === id)) return;
		void tick().then(() => {
			const slots = document.querySelectorAll<HTMLElement>('[data-record]');
			[...slots].find((el) => el.dataset.record === id)?.scrollIntoView({ block: 'center' });
		});
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

<div class="page" data-journey="dispatch-failures">
	<PageHead title={m.dispatch_failed_history()} />
	<div class="sub">
		<Text size="sm" tone="faint">{m.dispatch_failed_history_subtitle()}</Text>
		{#if undismissed > 1}
			<Button size="sm" variant="link" disabled={clearing} onclick={clearAll}>
				{m.dispatch_failed_dismiss_all()}
			</Button>
		{/if}
	</div>

	{#if history.isLoading}
		<div class="msg"><Spinner /></div>
	{:else if history.error}
		<div class="msg"><Text tone="danger">{errMessage(history.error)}</Text></div>
	{:else if rows.length === 0}
		<div class="msg"><Text tone="faint">{m.dispatch_failed_history_empty()}</Text></div>
	{:else}
		<div class="cards">
			{#each rows as d (d.session_id)}
				<div class="slot" class:focused={d.session_id === focused} data-record={d.session_id}>
					<FailedDispatchCard
						dispatch={d}
						onrerun={redispatch}
						ondismiss={d.dismissed ? undefined : dismiss}
					/>
				</div>
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
	.sub {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.cards {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.slot.focused {
		border-radius: var(--radius-md);
		box-shadow: 0 0 0 2px var(--accent);
	}
	.msg {
		display: grid;
		place-items: center;
		padding: var(--sp-6);
	}
</style>
