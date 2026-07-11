<script lang="ts">
	import type { Label } from '@bindings/Label';
	import type { Section } from '../../../routes/sessions/sessions.logic';
	import { Button, FilterSearchBar, Heading, Icon, type Schema } from '@dorsk/tsumikit';
	import { SESSION_SEARCH_PLACEHOLDER } from '$lib/searchSchema';
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
		searchSchema,
		sections = $bindable(),
		labels,
		labelFilter = $bindable(),
		cardView = $bindable(),
		dense = $bindable(),
		selecting,
		searching,
		onStartSelect,
		onCancelSelect,
		onNew,
		onUpdateLabel,
		onDeleteLabel
	}: {
		rawQuery: string;
		searchSchema: Schema;
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
		onUpdateLabel?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDeleteLabel?: (labelId: string) => void | Promise<void>;
	} = $props();
</script>

<div class="bar row">
	<Heading level={1} class="page-title">Sessions</Heading>
	<div class="search-box">
		<FilterSearchBar
			schema={searchSchema}
			bind:value={rawQuery}
			placeholder={SESSION_SEARCH_PLACEHOLDER}
		/>
	</div>
	<SectionFilter bind:sections />
	<LabelFilter {labels} bind:selected={labelFilter} onUpdate={onUpdateLabel} onDelete={onDeleteLabel} />
	<ViewPicker bind:cardView bind:dense />
	{#if !searching}
		{#if selecting}
			<!-- Cancel selection. -->
			<Button class="ctl btn-control-square" icon title="Cancel selection" aria-label="Cancel selection" onclick={onCancelSelect}>
				<Icon name="x" size={18} />
			</Button>
		{:else}
			<!-- "Select multiple" wants a checklist/multi-select glyph the registry
			     doesn't ship; feed Icon a raw list-checks svg via its children. -->
			<Button class="ctl btn-control-square" icon title="Select multiple to archive" aria-label="Select multiple to archive" onclick={onStartSelect}>
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
	<!-- Mobile-only flex row-break (CCT-369): basis:100% forces row 2 (search +
	     tools) onto a fresh line below title+New. Hidden on desktop where everything
	     sits on one row. -->
	<span class="row-break break-tools" aria-hidden="true"></span>
</div>

<style>
	.bar {
		/* Sticky under the fixed app header so the controls stay reachable on long
		   lists without scrolling back up (CCT-241). */
		position: sticky;
		top: calc(var(--header-h) + var(--safe-top));
		z-index: 6;
		margin-bottom: var(--sp-4);
		/* Pad the bottom only: top padding would drop the title below the header
		   baseline every other page aligns to. */
		padding: 0 0 var(--sp-2);
		gap: var(--sp-2);
		align-items: center;
		/* Wrap so controls reflow onto a second line instead of overflowing when
		   the UI scale grows the title/buttons. */
		flex-wrap: wrap;
		background: var(--bg);
	}
	/* The title is the Heading atom; target it via :global. Pinned to a fixed px
	   size (it's toolbar chrome, not content) so the UI font scale doesn't push
	   the action buttons out of frame. */
	:global(.page-title) {
		font-size: 28px;
		align-self: center;
		flex: none;
	}
	/* Search fills the gap between the title and the right-hand controls.
	   display:contents promotes the FilterSearchBar's bar and chips to sibling
	   flex items, so the chips wrap onto their own full-width row instead of
	   growing the first row — opening/typing must never move the input or the
	   surrounding controls (CCT-589 follow-up). */
	.bar :global(.search-box) {
		display: contents;
	}
	.bar :global(.search-box .fsb) {
		flex: 1 1 0;
		min-width: 0;
	}
	.bar :global(.search-box .fsb__chips) {
		flex: 0 0 100%;
		order: 5;
		margin-top: 0;
	}
	.bar :global(.toolbar-new) {
		flex: none;
	}
	.new-label {
		display: inline;
	}
	/* Row-break helpers: zero-height flex items with a full-width basis. Off by
	   default (single-row desktop bar); switched on in the mobile query below. */
	.row-break {
		display: none;
	}
	/* Mobile (CCT-369): two rows — row 1 title + full "+ New", row 2 the search bar
	   (which shrinks to fill) followed by the tool controls on the right. `order`
	   sequences the items; the row-break forces the single wrap. */
	@media (max-width: 639px) {
		/* Default everyone to row 2… */
		.bar > :global(*) {
			order: 2;
		}
		/* Row 1: title (grows to push New flush right) then the New button. */
		.bar > :global(.page-title) {
			order: 0;
			flex: 1 1 auto;
			min-width: 0;
		}
		.bar :global(.toolbar-new) {
			order: 1;
		}
		/* Break after row 1: forces search + tools onto row 2. height:0 + negative
		   row-gap cancel so the phantom line adds no vertical space. */
		.break-tools {
			display: block;
			order: 1;
			flex: 0 0 100%;
			height: 0;
			margin-top: calc(-1 * var(--sp-2));
		}
		/* Row 2: search takes only the leftover space (basis:0 so it never demands
		   its intrinsic input width) and shrinks freely; the tools follow on the
		   right, all on one row. The `.bar > *` order reset above can't reach the
		   display:contents children, so they're ordered here explicitly. */
		.bar :global(.search-box .fsb) {
			order: 2;
			flex: 1 1 0;
			min-width: 0;
		}
		.bar :global(.search-box .fsb__chips) {
			order: 5;
		}
	}
</style>
