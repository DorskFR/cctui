<script lang="ts">
	// List vs cards, as the kit's icon toggle. `cardView` is bindable so the
	// parent keeps owning persistence. In the overflow ⋯ menu it is a plain
	// full-width row like the dimension pickers; tapping it flips the view.
	import { Button, Icon, SegmentedControl } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		cardView = $bindable(),
		menu = false
	}: {
		cardView: boolean;
		/** Overflow ⋯ menu row: icon + "View: …", one tap toggles. */
		menu?: boolean;
	} = $props();

	const options = [
		{ value: 'list', icon: 'list' as const },
		{ value: 'card', icon: 'grid' as const }
	];
	// The menu row is an action, so it names the view it switches TO.
	const target = $derived(
		m.sessions_view_title({ view: cardView ? m.sessions_view_list() : m.sessions_view_card() })
	);
</script>

{#if menu}
	<Button
		variant="ghost"
		size="sm"
		block
		style="justify-content:flex-start"
		data-journey="view"
		title={target}
		onclick={() => (cardView = !cardView)}
	>
		<Icon name={cardView ? 'list' : 'grid'} size={18} />
		<span>{target}</span>
	</Button>
{:else}
	<span class="vp" data-journey="view">
		<SegmentedControl
			variant="icon"
			box
			label={m.sessions_view_label()}
			{options}
			bind:value={() => (cardView ? 'card' : 'list'), (v) => (cardView = v === 'card')}
		/>
	</span>
{/if}

<style>
	.vp {
		display: inline-flex;
	}
</style>
