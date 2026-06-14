<script lang="ts">
	import type { Label } from '@bindings/Label';
	import type { Section } from '../../../routes/sessions/sessions.logic';
	import { Button, Heading, Icon } from '@dorsk/tsumikit';
	import SearchBox from '../molecules/SearchBox.svelte';
	import SectionFilter from '../molecules/SectionFilter.svelte';
	import LabelFilter from '../molecules/LabelFilter.svelte';
	import ViewPicker from '../molecules/ViewPicker.svelte';

	// The sessions list toolbar (CCT-369): title + search + section/label filters +
	// view picker + multi-select toggle + New. A uniform, self-contained block —
	// each control is its own molecule so this layer only owns composition and the
	// responsive bar layout. Two-way state stays owned by the page (persisted to
	// drafts) and flows in via bindable props.
	let {
		rawQuery = $bindable(),
		sections = $bindable(),
		labels,
		labelFilter = $bindable(),
		cardView = $bindable(),
		dense = $bindable(),
		selecting,
		searching,
		onStartSelect,
		onCancelSelect,
		onNew
	}: {
		rawQuery: string;
		sections: Set<Section>;
		labels: Label[];
		labelFilter: Set<string>;
		cardView: boolean;
		dense: boolean;
		selecting: boolean;
		searching: boolean;
		onStartSelect: () => void;
		onCancelSelect: () => void;
		onNew: () => void;
	} = $props();
</script>

<div class="bar row">
	<Heading level={1} class="page-title">Sessions</Heading>
	<SearchBox bind:value={rawQuery} />
	<SectionFilter bind:sections />
	<LabelFilter {labels} bind:selected={labelFilter} />
	<ViewPicker bind:cardView bind:dense />
	{#if !searching}
		{#if selecting}
			<!-- Cancel selection. -->
			<Button class="ctl btn-control-square" icon variant="ghost" title="Cancel selection" aria-label="Cancel selection" onclick={onCancelSelect}>
				<Icon name="x" size={18} />
			</Button>
		{:else}
			<!-- "Select multiple" wants a checklist/multi-select glyph the registry
			     doesn't ship; feed Icon a raw list-checks svg via its children. -->
			<Button class="ctl btn-control-square" icon variant="ghost" title="Select multiple to archive" aria-label="Select multiple to archive" onclick={onStartSelect}>
				<Icon label="Select multiple to archive" size={18}>
					<path d="m3 17 2 2 4-4" />
					<path d="m3 7 2 2 4-4" />
					<path d="M13 6h8" />
					<path d="M13 12h8" />
					<path d="M13 18h8" />
				</Icon>
			</Button>
		{/if}
	{/if}
	<Button class="toolbar-new" control variant="primary" title="New session" aria-label="New session" onclick={onNew}>+<span class="new-label"> New</span></Button>
</div>

<style>
	.bar {
		/* Sticky under the fixed app header so the controls stay reachable on long
		   lists without scrolling back up (CCT-241). */
		position: sticky;
		top: calc(var(--header-h) + var(--safe-top));
		z-index: 6;
		margin-bottom: var(--sp-4);
		/* Only pad the bottom (CCT-314): symmetric top padding pushed the title
		   below every other page's header. */
		padding: 0 0 var(--sp-2);
		gap: var(--sp-2);
		align-items: center;
		/* Wrap so controls reflow onto a second line instead of overflowing when
		   the UI scale grows the title/buttons (CCT-308 item 3). */
		flex-wrap: wrap;
		background: var(--bg);
	}
	/* The title is the Heading atom; target it via :global. Pinned to a fixed px
	   size (toolbar chrome, not content) so the UI font scale doesn't shove the
	   action buttons out of frame (CCT-308 item 3). */
	:global(.page-title) {
		font-size: 28px;
		align-self: center;
		flex: none;
	}
	/* Search fills the gap between the title and the right-hand controls. */
	.bar :global(.search-box) {
		flex: 1;
		min-width: 0;
	}
	.bar :global(.toolbar-new) {
		flex: none;
	}
	.new-label {
		display: inline;
	}
	/* Mobile (CCT-369): row 1 holds the title + every square control + a square
	   "+" New button; the search input wraps onto its own full-width second row. */
	@media (max-width: 639px) {
		/* Title eats the leftover space on row 1 so the square controls pack to the
		   right edge instead of leaving a ragged gap. */
		.bar > :global(.page-title) {
			flex: 1 1 auto;
			min-width: 0;
		}
		/* Search drops to its own row, full width. `order` pushes it after every
		   row-1 control; the 100% basis forces the wrap. */
		.bar :global(.search-box) {
			order: 10;
			flex: 1 1 100%;
			width: 100%;
		}
		/* "+ New" collapses to just "+" so it matches the square aspect ratio of the
		   other controls and the whole set stays on one row. */
		.bar :global(.toolbar-new) {
			width: var(--control-height);
			padding: 0;
		}
		.new-label {
			display: none;
		}
	}
</style>
