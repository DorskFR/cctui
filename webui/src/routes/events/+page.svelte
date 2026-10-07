<script lang="ts">
	import { Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import EventFeed from '$lib/components/organisms/events/EventFeed.svelte';
	import EventFilterBar from '$lib/components/organisms/events/EventFilterBar.svelte';
	import { DEFAULT_FILTERS, type EventFilters } from '$lib/events';
	import { useMe } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';

	let filters = $state<EventFilters>({ ...DEFAULT_FILTERS });
	const me = useMe();
	const admin = $derived(me.data?.scopes?.includes('admin') ?? false);
</script>

<div class="page">
	<PageHead title={m.events_title()} />
	<Text size="sm" tone="faint">{m.events_subtitle()}</Text>
	<EventFilterBar bind:filters {admin} />
	<EventFeed {filters} />
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		padding: var(--sp-4);
		max-width: 72rem;
		width: 100%;
		margin: 0 auto;
	}
</style>
