<script lang="ts">
	import type { Label } from '@bindings/Label';
	import type { Section } from '../../../routes/sessions/sessions.logic';
	import {
		Button,
		Field,
		FilterSearchBar,
		Heading,
		Icon,
		type Schema,
		Text
	} from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { sessionSearchPlaceholder } from '$lib/searchSchema';
	import SectionFilter from '../molecules/SectionFilter.svelte';
	import LabelFilter from '../molecules/LabelFilter.svelte';
	import ViewPicker from '../molecules/ViewPicker.svelte';
	import DimensionPicker from '../molecules/DimensionPicker.svelte';
	import type { Dimension } from '../../../routes/sessions/sessions.logic';

	// The sessions list toolbar: title + search + section/label filters +
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
		kanban = $bindable(),
		colorBy,
		groupBy,
		onColorBy,
		onGroupBy,
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
		kanban: boolean;
		colorBy: Dimension;
		groupBy: Dimension;
		onColorBy: (v: Dimension) => void;
		onGroupBy: (v: Dimension) => void;
		selecting: boolean;
		searching: boolean;
		onStartSelect: () => void;
		onCancelSelect: () => void;
		// Absent when the docked spawn panel replaces the "+ New" button.
		onNew?: () => void;
		onUpdateLabel?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDeleteLabel?: (labelId: string) => void | Promise<void>;
	} = $props();

	const searchId = $props.id();

	// Overflow menu: the toolbar grew too many buttons and squeezed the
	// search bar. A ⋯ flyout collapses the secondary controls. On desktop it holds
	// the two DimensionPickers (color-by · group-by) so the search bar reclaims
	// width; on narrow widths a container query also folds the label/view/select
	// controls in, leaving only the section filter inline. Width-driven (container
	// query) not viewport-driven, mirroring DrawerHeader.
	let moreOpen = $state(false);

	function closeMoreFromOutside(e: PointerEvent) {
		if (!moreOpen) return;
		const t = e.target as HTMLElement | null;
		if (t?.closest('.secondary') || t?.closest('.more')) return;
		moreOpen = false;
	}
	function onWinKey(e: KeyboardEvent) {
		if (e.key === 'Escape' && moreOpen) moreOpen = false;
	}
</script>

<svelte:window onkeydown={onWinKey} onpointerdown={closeMoreFromOutside} />

<!-- Controls that stay inline on desktop but fold into the ⋯ flyout on narrow
     widths: label filter, view picker, multi-select toggle. Rendered
     via a snippet so the inline copy and the flyout copy share one source. -->
{#snippet listChecks()}
	<Icon label={m.sessions_select_multiple()} size={18}>
		<path d="m3 17 2 2 4-4" />
		<path d="m3 7 2 2 4-4" />
		<path d="M13 6h8" />
		<path d="M13 12h8" />
		<path d="M13 18h8" />
	</Icon>
{/snippet}
{#snippet foldControls(menu: boolean)}
	<LabelFilter {menu} {labels} bind:selected={labelFilter} onUpdate={onUpdateLabel} onDelete={onDeleteLabel} />
	<ViewPicker {menu} bind:cardView bind:kanban />
	<!-- Stays mounted (disabled) while searching: unmounting it re-wraps the
	     flex bar mid-type and makes the search field jump. -->
	{#if selecting}
		<!-- Cancel selection. -->
		{#if menu}
			<button type="button" class="menu-trigger" onclick={onCancelSelect}>
				<Icon name="x" size={18} /><span>{m.sessions_cancel_selection()}</span>
			</button>
		{:else}
			<Button class="ctl" square title={m.sessions_cancel_selection()} aria-label={m.sessions_cancel_selection()} onclick={onCancelSelect}>
				<Icon name="x" size={18} />
			</Button>
		{/if}
	{:else if menu}
		<button type="button" class="menu-trigger" disabled={searching} onclick={onStartSelect}>
			{@render listChecks()}<span>{m.sessions_select_multiple()}</span>
		</button>
	{:else}
		<!-- "Select multiple" wants a checklist/multi-select glyph the registry
		     doesn't ship; feed Icon a raw list-checks svg via its children. -->
		<Button class="ctl" square disabled={searching} title={m.sessions_select_multiple()} aria-label={m.sessions_select_multiple()} onclick={onStartSelect}>
			{@render listChecks()}
		</Button>
	{/if}
{/snippet}

<div class="bar row">
	<Heading level={1} class="sess-title">{m.sessions_title()}</Heading>
	<!-- FilterSearchBar forwards no id/aria-label, so the name reaches its input
	     through the Field context; the label itself is screen-reader only. -->
	<div class="search-box">
		<Text as="label" for={searchId} class="sr-only">{m.a11y_sessions_search()}</Text>
		<Field for={searchId}>
			<FilterSearchBar
				schema={searchSchema}
				bind:value={rawQuery}
				placeholder={sessionSearchPlaceholder()}
			/>
		</Field>
	</div>
	<SectionFilter bind:sections />
	<!-- Inline copy of the foldable controls: visible on desktop, hidden by the
	     container query below (where the flyout copy takes over). display:contents
	     so each control stays a direct flex item of the bar. -->
	<div class="inline-fold">{@render foldControls(false)}</div>
	<!-- ⋯ overflow flyout, anchored to its own trigger. The wrapper is the
	     positioning context (not the wrapping/container-scoped bar), so the menu
	     drops directly under the ⋯ button at every width instead of detaching to
	     the bar's far edge. -->
	<div class="more-wrap">
		<Button
			class="more"
			square
			aria-label={m.drawer_more_actions()}
			title={m.drawer_more_actions()}
			aria-expanded={moreOpen}
			onclick={() => (moreOpen = !moreOpen)}
		>
			<Icon name="more" size={18} />
		</Button>
		<!-- The two DimensionPickers live here at all widths; narrow widths also
		     receive the foldable controls (menu-only copy). -->
		<div class="secondary" class:open={moreOpen}>
			<div class="menu-fold">{@render foldControls(true)}</div>
			<DimensionPicker menu kind="group" value={groupBy} onchange={onGroupBy} />
			<DimensionPicker menu kind="color" value={colorBy} onchange={onColorBy} />
		</div>
	</div>
	{#if onNew}
		<Button class="toolbar-new" variant="primary" title={m.sessions_new_session()} aria-label={m.sessions_new_session()} onclick={onNew}>+<span class="new-label"> {m.sessions_new()}</span></Button>
	{/if}
	<!-- Mobile-only flex row-break: basis:100% forces row 2 (search +
	     tools) onto a fresh line below title+New. Hidden on desktop where everything
	     sits on one row. -->
	<span class="row-break break-tools" aria-hidden="true"></span>
</div>

<style>
	.bar {
		/* Sticky under the fixed app header so the controls stay reachable on long
		   lists without scrolling back up. */
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
		/* Fold the secondary controls based on the bar's own width, not the viewport
		  , mirroring DrawerHeader. The bar is already position:sticky, so
		   it also serves as the positioning context for the absolute flyout. */
		container: sess-bar / inline-size;
	}
	/* Inline copy of the foldable controls flows as bar-level flex items. */
	.inline-fold {
		display: contents;
	}
	/* The ⋯ trigger + its flyout share one positioning context so the menu drops
	   under the button, not the wrapping bar's far edge. */
	.more-wrap {
		position: relative;
		flex: none;
		display: flex;
	}
	/* ⋯ flyout: an absolute dropdown anchored to the ⋯ button, holding the
	   DimensionPickers at all widths (plus the foldable controls on narrow ones).
	   Hidden until opened. */
	.secondary {
		display: none;
		position: absolute;
		top: calc(100% + var(--sp-1));
		right: 0;
		/* Fixed, content-comfortable width so the labeled rows read as a real menu
		   (mirrors the drawer's ⋯ flyout), never exceeding the viewport. */
		width: 15rem;
		max-width: calc(100vw - 1.5rem);
		z-index: 30;
		flex-direction: column;
		align-items: stretch;
		gap: 2px;
		padding: var(--sp-1);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		box-shadow: var(--shadow-lg, 0 8px 24px rgba(0, 0, 0, 0.5));
	}
	.secondary.open {
		display: flex;
	}
	/* The foldable controls sit in the flyout ONLY on narrow widths; on desktop
	   they render inline (via .inline-fold) so the menu holds just the dimensions. */
	.menu-fold {
		display: none;
	}
	/* Local action rows (select-multiple / cancel) inside the flyout: plain icon +
	   label, matching the picker rows' menu-row look. */
	.menu-trigger {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		width: 100%;
		min-height: 2.25rem;
		padding: var(--sp-1) var(--sp-2);
		border: none;
		background: none;
		border-radius: var(--r-sm);
		color: var(--text);
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		text-align: left;
		cursor: pointer;
	}
	.menu-trigger:hover {
		background: var(--bg-elevated-3, var(--bg-elevated-2));
	}
	.menu-trigger:disabled {
		opacity: 0.5;
		cursor: default;
	}
	.menu-trigger:disabled:hover {
		background: none;
	}
	@container sess-bar (max-width: 640px) {
		.inline-fold {
			display: none;
		}
		.menu-fold {
			display: contents;
		}
	}
	/* The title is the Heading atom; target it via :global. Pinned to a fixed px
	   size (it's toolbar chrome, not content) so the UI font scale doesn't push
	   the action buttons out of frame. */
	.bar > :global(.sess-title) {
		font-size: 28px;
		align-self: center;
		flex: none;
	}
	/* Search fills the gap between the title and the right-hand controls. Our
	   own wrapper is the flex item and is sized directly, so the FilterSearchBar
	   root fills it (block, width:100%) and its below-bar chips stack onto their
	   own row within the wrapper — opening/typing never moves the input or the
	   surrounding controls. Sized here, not via any library
	   internal class, so a tsumikit internal-class rename can't break it. */
	.search-box {
		flex: 1 1 0;
		min-width: 0;
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
	/* Narrow bar: two rows — row 1 title + full "+ New", row 2
	   the search bar (which shrinks to fill) followed by the tool controls on the
	   right. `order` sequences the items; the row-break forces the single wrap.
	   Driven by the SAME container query as the fold above (not a viewport media
	   query): the bar is often narrower than the viewport, so a viewport breakpoint
	   left a dead band where controls folded but the reorg didn't fire, orphaning
	   the tools onto a lonely row while search stayed cramped on row 1. */
	@container sess-bar (max-width: 640px) {
		/* Default everyone to row 2… */
		.bar > :global(*) {
			order: 2;
		}
		/* Row 1: title (grows to push New flush right) then the New button. */
		.bar > :global(.sess-title) {
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
		/* Row 2: search takes only the leftover space (flex:1 1 0 from the base
		   rule, so it never demands its intrinsic input width) and shrinks freely;
		   the tools follow on the right, all on one row. It picks up order:2 from
		   the `.bar > *` reset above as a real (non-contents) flex item. */
	}
</style>
