<script lang="ts">
	// List vs cards, as the kit's icon toggle. `cardView` is bindable so the
	// parent keeps owning persistence.
	import { SegmentedControl } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		cardView = $bindable(),
		menu = false
	}: {
		cardView: boolean;
		/** Overflow ⋯ menu row: the same toggle, one size down. */
		menu?: boolean;
	} = $props();

	const options = [
		{ value: 'list', icon: 'list' as const },
		{ value: 'card', icon: 'grid' as const }
	];
</script>

<SegmentedControl
	variant="icon"
	size={menu ? 'sm' : 'md'}
	label={m.sessions_view_label()}
	{options}
	bind:value={() => (cardView ? 'card' : 'list'), (v) => (cardView = v === 'card')}
/>
