<script lang="ts">
	import type { EventPage } from '@bindings/EventPage';
	import type { EventRecord } from '@bindings/EventRecord';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Button, EmptyState, Spinner, Text } from '@dorsk/tsumikit';
	import EventRow from './EventRow.svelte';
	import { EVENT_PAGE, endpoints, qk, useEvents } from '$lib/queries';
	import { appendPage, filterQuery, matchesFilters, mergeLive, nextCursor, type EventFilters } from '$lib/events';
	import { ws } from '$lib/ws.svelte';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';

	let { filters }: { filters: EventFilters } = $props();

	const query = $derived(filterQuery(filters));
	const first = useEvents(() => query);
	const qc = useQueryClient();

	type Extra = {
		key: string;
		rows: EventRecord[];
		state: 'idle' | 'loading' | 'error' | 'done';
		error: string | null;
	};
	const key = $derived(JSON.stringify(query));
	const fresh = (k: string): Extra => ({ key: k, rows: [], state: 'idle', error: null });
	// Pages past the first hang under the filter key they were fetched for; a
	// filter change leaves them behind without an effect resetting anything.
	let fetched = $state<Extra>(fresh(''));
	const more = $derived(fetched.key === key ? fetched : fresh(key));

	const rows = $derived(appendPage(first.data?.events ?? [], more.rows));
	const hasMore = $derived(
		more.state === 'done' ? false : more.rows.length > 0 ? true : (first.data?.has_more ?? false)
	);

	$effect(() =>
		ws.onEvent((ev) => {
			if (!matchesFilters(ev, filters)) return;
			qc.setQueryData<EventPage>(qk.events(query), (old) =>
				old ? { ...old, events: mergeLive(old.events, ev) } : { events: [ev], has_more: false }
			);
		})
	);

	async function loadMore() {
		const before = nextCursor(rows);
		const k = key;
		if (before === null || more.state === 'loading') return;
		fetched = { ...more, key: k, state: 'loading', error: null };
		try {
			const page = await endpoints.events({ ...query, before: String(before), limit: String(EVENT_PAGE) });
			if (key !== k) return;
			fetched = {
				key: k,
				rows: appendPage(fetched.rows, page.events),
				state: page.has_more ? 'idle' : 'done',
				error: null
			};
		} catch (e) {
			if (key !== k) return;
			fetched = { ...fetched, key: k, state: 'error', error: errMessage(e) };
		}
	}
</script>

<section class="feed" aria-live="polite" data-journey="events-feed">
	{#if first.isPending}
		<div class="msg"><Spinner /></div>
	{:else if first.isError}
		<div class="msg"><Text size="sm" tone="danger">{errMessage(first.error)}</Text></div>
	{:else if rows.length === 0}
		<EmptyState icon="clock" title={m.events_empty_title()} description={m.events_empty_body()} size="compact" />
	{:else}
		<ul class="list">
			{#each rows as ev (ev.id)}
				<EventRow event={ev} />
			{/each}
		</ul>
		{#if hasMore}
			<div class="more">
				{#if more.state === 'error'}
					<Text size="xs" tone="danger">{more.error}</Text>
				{/if}
				<Button size="sm" variant="ghost" onclick={loadMore} disabled={more.state === 'loading'}>
					{more.state === 'loading' ? m.events_loading_more() : m.events_load_more()}
				</Button>
			</div>
		{/if}
	{/if}
</section>

<style>
	.feed {
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		overflow: hidden;
	}
	.list {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.msg {
		display: grid;
		place-items: center;
		padding: var(--sp-6);
	}
	.more {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--sp-2);
		padding: var(--sp-2);
		border-top: 1px solid var(--border);
	}
</style>
