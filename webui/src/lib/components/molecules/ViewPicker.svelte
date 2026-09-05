<script lang="ts">
	// List vs cards, as the kit's icon toggle. `cardView` is bindable so the
	// parent keeps owning persistence. In the overflow ⋯ menu it is a plain
	// full-width row like the dimension pickers; tapping it flips the view.
	import { Icon, SegmentedControl } from '@dorsk/tsumikit';
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
	const title = $derived(
		m.sessions_view_title({ view: cardView ? m.sessions_view_card() : m.sessions_view_list() })
	);
</script>

{#if menu}
	<button type="button" class="menu-row" {title} onclick={() => (cardView = !cardView)}>
		<Icon name={cardView ? 'grid' : 'list'} size={18} />
		<span>{title}</span>
	</button>
{:else}
	<SegmentedControl
		variant="icon"
		size="md"
		label={m.sessions_view_label()}
		{options}
		bind:value={() => (cardView ? 'card' : 'list'), (v) => (cardView = v === 'card')}
	/>
{/if}

<style>
	.menu-row {
		display: flex;
		align-items: center;
		width: 100%;
		justify-content: flex-start;
		gap: var(--sp-2);
		min-height: 2.25rem;
		padding: var(--sp-1) var(--sp-2);
		border: 0;
		border-radius: var(--r-sm);
		background: none;
		color: inherit;
		font: inherit;
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		white-space: nowrap;
		cursor: pointer;
	}
	.menu-row:hover {
		background: var(--bg-elevated-3, var(--bg-elevated-2));
	}
</style>
