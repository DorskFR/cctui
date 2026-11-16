<script lang="ts">
	import { Badge, Card, Link, Spinner, Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import { errMessage } from '$lib/api';
	import {
		INBOX_BUCKETS,
		groupInboxByLane,
		useCancelInboxEvent,
		useInboxEvents,
		useRetryInboxEvent,
		type InboxBucket
	} from '$lib/queries/inbox';
	import { sessionHref } from '$lib/sessionFailureToast';
	import { m } from '$lib/paraglide/messages';
	import InboxEventRow from './InboxEventRow.svelte';

	const events = useInboxEvents();
	const cancel = useCancelInboxEvent();
	const retry = useRetryInboxEvent();
	const lanes = $derived(groupInboxByLane(events.data ?? []));

	const label: Record<InboxBucket, () => string> = {
		pending: m.inbox_status_pending,
		dead: m.inbox_status_dead,
		delivered: m.inbox_status_delivered,
		cancelled: m.inbox_status_cancelled
	};
	const tone: Record<InboxBucket, 'warn' | 'danger' | 'ok' | 'muted'> = {
		pending: 'warn',
		dead: 'danger',
		delivered: 'ok',
		cancelled: 'muted'
	};
</script>

<div class="page" data-journey="inbox">
	<PageHead title={m.inbox_title()} />
	<Text size="sm" tone="faint">{m.inbox_subtitle()}</Text>

	{#if events.isLoading}
		<div class="msg"><Spinner /></div>
	{:else if events.error}
		<div class="msg"><Text tone="danger">{errMessage(events.error)}</Text></div>
	{:else if lanes.length === 0}
		<div class="msg"><Text tone="faint">{m.inbox_empty()}</Text></div>
	{:else}
		<div class="lanes">
			{#each lanes as lane (lane.id)}
				<Card padding="sm" data-journey="inbox-lane">
					<div class="lane-head">
						{#if lane.sessionId}
							<Link href={sessionHref(lane.sessionId)}>
								<Text weight="bold" variant="code">{lane.key}</Text>
							</Link>
						{:else}
							<Text weight="bold" variant="code">{lane.key}</Text>
						{/if}
						{#each INBOX_BUCKETS as bucket (bucket)}
							{#if lane.buckets[bucket].length}
								<Badge size="xs" tone={tone[bucket]} border>
									{label[bucket]()} · {lane.buckets[bucket].length}
								</Badge>
							{/if}
						{/each}
					</div>
					{#each INBOX_BUCKETS as bucket (bucket)}
						{#if lane.buckets[bucket].length}
							<section class="bucket" data-bucket={bucket}>
								<Text size="xs" tone="faint" weight="bold">{label[bucket]()}</Text>
								{#each lane.buckets[bucket] as event (event.id)}
									<InboxEventRow {event} oncancel={cancel} onretry={retry} />
								{/each}
							</section>
						{/if}
					{/each}
				</Card>
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
	.lanes {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.lane-head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}
	.bucket {
		display: flex;
		flex-direction: column;
		margin-top: var(--sp-2);
	}
	.msg {
		display: grid;
		place-items: center;
		padding: var(--sp-6);
	}
</style>
