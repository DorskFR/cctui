<script lang="ts">
	import type { EventRecord } from '@bindings/EventRecord';
	import { Disclosure, Dot, Text, Timestamp } from '@dorsk/tsumikit';
	import EventRow from './EventRow.svelte';
	import { m } from '$lib/paraglide/messages';

	let { events, allChildren }: { events: EventRecord[]; allChildren: boolean } = $props();

	const latest = $derived(events[0]);
</script>

<li class="row" data-kind="session.created" data-folded={events.length}>
	<Disclosure>
		{#snippet header()}
			<span class="head">
				<Dot status="active" role="img" aria-label="info" title="info" />
				<Timestamp value={latest.occurred_at} mode="relative" short size="xs" tone="faint" />
				<Text variant="eyebrow" tone="muted" nowrap>{m.events_family_session()}</Text>
				<Text size="sm">
					{allChildren
						? m.set_events_children_created({ count: events.length })
						: m.set_events_sessions_created({ count: events.length })}
				</Text>
			</span>
		{/snippet}
		<ul class="children">
			{#each events as ev (ev.id)}
				<EventRow event={ev} compact />
			{/each}
		</ul>
	</Disclosure>
</li>

<style>
	.row {
		padding: var(--sp-1) var(--sp-3);
		border-bottom: 1px solid var(--border-subtle, var(--border));
	}
	.row:last-child {
		border-bottom: 0;
	}
	.head {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.children {
		list-style: none;
		margin: 0;
		padding: 0 0 var(--sp-1) var(--sp-4);
	}
</style>
