<script lang="ts">
	// List / cards / tiles, as the kit's icon toggle. `view` is bindable so the
	// parent keeps owning persistence. In the overflow ⋯ menu it is a plain
	// full-width row like the dimension pickers; tapping it advances the view.
	import { Button, Icon, SegmentedControl, type IconName } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import type { ViewMode } from '../../../routes/sessions/sessionsPage.svelte';

	let {
		view = $bindable(),
		tiles = true,
		menu = false
	}: {
		view: ViewMode;
		/** Tiles need a window to split, so phones are not offered the option. */
		tiles?: boolean;
		/** Overflow ⋯ menu row: icon + "View: …", one tap advances. */
		menu?: boolean;
	} = $props();

	const ICONS: Record<ViewMode, IconName> = {
		list: 'list',
		grid: 'grid',
		tiles: 'viewport'
	};
	const LABELS: Record<ViewMode, () => string> = {
		list: () => m.sessions_view_list(),
		grid: () => m.sessions_view_card(),
		tiles: () => m.sessions_view_tiles()
	};

	const modes: ViewMode[] = $derived(tiles ? ['list', 'grid', 'tiles'] : ['list', 'grid']);
	const options = $derived(modes.map((value) => ({ value, icon: ICONS[value] })));
	const next = $derived(modes[(modes.indexOf(view) + 1) % modes.length] ?? 'list');
	// The menu row is an action, so it names the view it switches TO.
	const target = $derived(m.sessions_view_title({ view: LABELS[next]() }));
</script>

{#if menu}
	<Button
		variant="ghost"
		size="sm"
		block
		style="justify-content:flex-start"
		data-journey="view"
		title={target}
		onclick={() => (view = next)}
	>
		<Icon name={ICONS[next]} size={18} />
		<span>{target}</span>
	</Button>
{:else}
	<span class="vp" data-journey="view">
		<SegmentedControl
			variant="icon"
			box
			label={m.sessions_view_label()}
			{options}
			bind:value={() => view, (v) => (view = v as ViewMode)}
		/>
	</span>
{/if}

<style>
	.vp {
		display: inline-flex;
	}
</style>
