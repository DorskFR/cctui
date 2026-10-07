<script lang="ts">
	import type { EventPage } from '@bindings/EventPage';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Disclosure, Text } from '@dorsk/tsumikit';
	import EventRow from '../events/EventRow.svelte';
	import { qk, useSessionEvents } from '$lib/queries';
	import { mergeLive, sessionIdOf } from '$lib/events';
	import { ws } from '$lib/ws.svelte';
	import { m } from '$lib/paraglide/messages';

	let { sessionId }: { sessionId: string } = $props();

	let open = $state(false);
	const events = useSessionEvents(
		() => sessionId,
		() => open
	);
	const qc = useQueryClient();
	const rows = $derived(events.data?.events ?? []);
	const last = $derived(rows[0] ?? null);

	$effect(() =>
		ws.onEvent((ev) => {
			if (sessionIdOf(ev) !== sessionId) return;
			qc.setQueryData<EventPage>(qk.sessionEvents(sessionId), (old) =>
				old ? { ...old, events: mergeLive(old.events, ev) } : { events: [ev], has_more: false }
			);
		})
	);
</script>

<div class="events" data-journey="session-events">
	<Disclosure chevron="start" size="compact" bind:open>
		{#snippet header()}
			<span class="strip-text">
				<span class="title">{m.events_panel_heading()}</span>
				{#if open && rows.length}
					<span class="count">{rows.length}</span>
				{:else if last}
					<span class="now" title={last.summary}>{last.summary}</span>
				{/if}
			</span>
		{/snippet}
		{#if open}
			{#if events.isPending}
				<div class="pad"><Text size="xs" tone="faint">{m.common_loading()}</Text></div>
			{:else if events.isError}
				<div class="pad"><Text size="xs" tone="danger">{m.events_panel_error()}</Text></div>
			{:else if rows.length === 0}
				<div class="pad"><Text size="xs" tone="faint">{m.events_panel_empty()}</Text></div>
			{:else}
				<ul class="list">
					{#each rows as ev (ev.id)}
						<EventRow event={ev} compact />
					{/each}
				</ul>
			{/if}
		{/if}
	</Disclosure>
</div>

<style>
	.events {
		flex: none;
		min-width: 0;
		max-width: 100%;
		border-bottom: 1px solid var(--border-subtle, var(--border));
		font-size: var(--fs-xs);
		overflow: hidden;
	}
	.strip-text {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
		min-width: 0;
		color: var(--text-muted);
		font-size: var(--fs-xs);
		font-weight: var(--fw-normal);
	}
	.title {
		flex: none;
	}
	.count {
		flex: none;
		color: var(--text-faint);
		font-variant-numeric: tabular-nums;
	}
	.now {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-faint);
	}
	.list {
		list-style: none;
		margin: 0;
		padding: 0 var(--sp-2) var(--sp-2) var(--sp-2);
		max-height: 32vh;
		overflow-y: auto;
		overflow-x: hidden;
	}
	.pad {
		padding: 0 var(--sp-3) var(--sp-2);
	}
</style>
