<script lang="ts">
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import EventFeed from './EventFeed.svelte';
	import EventFilterBar from './EventFilterBar.svelte';
	import { DEFAULT_FILTERS, type EventFilters } from '$lib/events';
	import { useMe } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';

	let filters = $state<EventFilters>({ ...DEFAULT_FILTERS });
	const me = useMe();
	const admin = $derived(me.data?.scopes?.includes('admin') ?? false);
</script>

<div class="panel" data-journey="page" data-journey-key="events">
	<PageHead title={m.events_title()} subtitle={m.events_subtitle()} />
	<EventFilterBar bind:filters {admin} />
	<EventFeed {filters} />
</div>

<style>
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}
</style>
