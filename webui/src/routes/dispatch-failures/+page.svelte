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
	import { useMe, useUserDispatchers } from '#lib/queries/index.ts';
	import { settingsHref } from '#lib/components/organisms/settings/settings.logic.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { groupByCause, type CauseGroup } from './groups.logic';

	const history = useFailedDispatchHistory();
	const redispatch = useRedispatch();
	const dismiss = useDismissFailedDispatch();
	const dismissAll = useDismissAllFailedDispatches();
	const rows = $derived(history.data ?? []);
	const groups = $derived(groupByCause(rows));
	const me = useMe();
	const mine = useUserDispatchers();
	const managed = $derived(
		me.data?.role === 'admin' ? new Set((mine.data ?? []).map((d) => d.name)) : new Set<string>()
	);
	const dispatcherHref = (name: string) =>
		managed.has(name) ? settingsHref('instance', 'instance-dispatchers') : null;

	function causeText(g: CauseGroup): string {
		switch (g.kind) {
			case 'env':
				return m.dispatch_failed_rerun_env({ keys: g.envKeys.join(', ') });
			case 'prompt_truncated':
				return m.dispatch_failed_rerun_prompt();
			case 'prompt_redacted':
				return m.dispatch_failed_rerun_redacted();
			case 'payload_too_large':
				return m.dispatch_failed_rerun_payload();
			case 'resolved':
				return m.dispatch_failed_group_resolved();
			default:
				return m.dispatch_failed_rerun_note();
		}
	}
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
	<header class="intro">
		<PageHead title={m.dispatch_failed_history()} />
		<div class="sub">
			<Text size="sm" tone="faint" measure>{m.dispatch_failed_history_subtitle()}</Text>
			{#if undismissed > 1}
				<Button size="sm" variant="link" disabled={clearing} onclick={clearAll}>
					{m.dispatch_failed_dismiss_all()}
				</Button>
			{/if}
		</div>
	</header>

	{#if history.isLoading}
		<div class="msg"><Spinner /></div>
	{:else if history.error}
		<div class="msg"><Text tone="danger">{errMessage(history.error)}</Text></div>
	{:else if rows.length === 0}
		<div class="msg"><Text tone="faint">{m.dispatch_failed_history_empty()}</Text></div>
	{:else}
		{#each groups as g (g.key)}
			<section class="group" data-cause={g.kind}>
				<header class="group-head">
					<Text as="p" size="sm" tone="muted" measure>{causeText(g)}</Text>
					<Text as="span" size="xs" tone="faint">{m.dispatch_failed_group_count({ count: g.rows.length })}</Text>
				</header>
				<div class="cards">
					{#each g.rows as d (d.session_id)}
						<div class="slot" class:focused={d.session_id === focused} data-record={d.session_id}>
							<FailedDispatchCard
								dispatch={d}
								dispatcherHref={dispatcherHref(d.dispatcher)}
								expanded={d.session_id === focused}
								onrerun={redispatch}
								ondismiss={d.dismissed ? undefined : dismiss}
							/>
						</div>
					{/each}
				</div>
			</section>
		{/each}
	{/if}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.intro {
		display: flex;
		flex-direction: column;
		margin-bottom: var(--sp-3);
	}
	.sub {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		margin-bottom: var(--sp-4);
	}
	.group-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--sp-3);
	}
	.cards {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
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
