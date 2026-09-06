<script lang="ts">
	// Conversation toolbar: three visually-separated control groups —
	// message-category filter, formatting toggles, behavior (auto-approve)
	// toggle — that on mobile collapse behind three text-button tabs opening
	// popovers.
	import {
		MSG_CATEGORIES,
		QUICK_FILTERS,
		allFilter,
		quickOn,
		quickPartial,
		withQuick
	} from './filters';
	import FilterMenu from './FilterMenu.svelte';
	import { quickFilterLabel, type MsgCategory, type QuickFilterId, type ViewOpts } from './types';
	import { Popover, Toggle } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		view = $bindable(),
		autoApprove,
		mobilePanel = $bindable(),
		ontoggleAuto,
		ondiagnose,
		onterminal,
		terminalOpen = false
	}: {
		view: ViewOpts;
		autoApprove: boolean;
		mobilePanel: 'filters' | 'format' | 'auto' | null;
		ontoggleAuto: () => void;
		/** Opens the session diagnose panel; omit to hide the button. */
		ondiagnose?: () => void;
		/** Toggles the read-only live terminal; omit to hide (codex). */
		onterminal?: () => void;
		terminalOpen?: boolean;
	} = $props();

	const QUICK_TINT: Record<QuickFilterId, string> = {
		assistant: 'var(--role-assistant)',
		user: 'var(--role-user)',
		tools: 'var(--role-tool)'
	};

	const offCount = $derived(MSG_CATEGORIES.filter((c) => !view.msgFilter[c]).length);

	function quickTitle(id: QuickFilterId): string {
		const label = quickFilterLabel(id);
		const state = quickOn(view.msgFilter, id)
			? m.conversation_filter_state_shown()
			: quickPartial(view.msgFilter, id)
				? m.conversation_filter_state_partial()
				: m.conversation_filter_state_hidden();
		return m.conversation_filter_quick_title({ label, state });
	}

	function toggleQuick(id: QuickFilterId) {
		view.msgFilter = withQuick(view.msgFilter, id, !quickOn(view.msgFilter, id));
	}

	function toggleCategory(c: MsgCategory) {
		view.msgFilter = { ...view.msgFilter, [c]: !view.msgFilter[c] };
	}

	function togglePanel(p: 'filters' | 'format' | 'auto') {
		mobilePanel = mobilePanel === p ? null : p;
	}
</script>

<div class="toolbar" class:panel-active={mobilePanel !== null}>
	<!-- Mobile: collapse the three control groups into a single row
	     of text buttons that each open a popover. Hidden on desktop, where the
	     groups render inline below. -->
	<div class="mobile-tabs" role="group" aria-label={m.conversation_chat_controls_aria()}>
		<Toggle
			class="mtab"
			data-journey="mobile-panel"
			data-journey-key="filters"
			pressed={mobilePanel === 'filters'}
			aria-expanded={mobilePanel === 'filters'}
			onclick={() => togglePanel('filters')}>{m.conversation_filters()}</Toggle
		>
		<Toggle
			class="mtab"
			data-journey="mobile-panel"
			data-journey-key="format"
			pressed={mobilePanel === 'format'}
			aria-expanded={mobilePanel === 'format'}
			onclick={() => togglePanel('format')}>{m.conversation_format()}</Toggle
		>
		<Toggle
			class="mtab"
			data-journey="mobile-panel"
			data-journey-key="auto"
			pressed={mobilePanel === 'auto' || autoApprove}
			style={autoApprove ? '--toggle-accent: var(--warn)' : ''}
			aria-expanded={mobilePanel === 'auto'}
			onclick={() => togglePanel('auto')}>{m.conversation_auto_approve_tab()}</Toggle
		>
	</div>
	<!-- Quick category toggles + the full per-category picker. A quick chip is
	     "on" only when every category it covers is on; a partly-on group gets a
	     dashed border instead. -->
	<div class="tagbar row row-wrap" data-journey="filters" class:panel-open={mobilePanel === 'filters'} role="group" aria-label={m.conversation_msg_filter_aria()}>
		{#each QUICK_FILTERS as q (q.id)}
			<Toggle
				pill
				data-journey="quick"
				data-journey-key={q.id}
				pressed={quickOn(view.msgFilter, q.id)}
				style={`--toggle-accent: ${QUICK_TINT[q.id]}${quickPartial(view.msgFilter, q.id) ? ';border-style:dashed' : ''}`}
				title={quickTitle(q.id)}
				onclick={() => toggleQuick(q.id)}
			>
				{quickFilterLabel(q.id)}
			</Toggle>
		{/each}
		<Popover label={m.conversation_filter_menu_aria()} placement="bottom-start" bare triggerClass="filters-trigger">
			{#snippet trigger()}
				{offCount > 0
					? m.conversation_filters_off_count({ count: offCount })
					: m.conversation_filters()}
			{/snippet}
			<FilterMenu
				filter={view.msgFilter}
				ontoggle={toggleCategory}
				onall={(on) => (view.msgFilter = allFilter(on))}
			/>
		</Popover>
	</div>
	<!-- Formatting toggles: gray when off, colored when on. -->
	<div class="fmtbar row row-wrap" class:panel-open={mobilePanel === 'format'} role="group" aria-label={m.conversation_formatting_aria()}>
		<Toggle pressed={view.prettyJson} onclick={() => (view.prettyJson = !view.prettyJson)}>{m.conversation_fmt_json()}</Toggle>
		<Toggle pressed={view.prettyDiff} onclick={() => (view.prettyDiff = !view.prettyDiff)}>{m.conversation_fmt_diff()}</Toggle>
		<Toggle pressed={view.prettyTables} onclick={() => (view.prettyTables = !view.prettyTables)} title={m.conversation_fmt_tables_title()}>{m.conversation_fmt_tables()}</Toggle>
	</div>
	<!-- Behavior toggle: distinct from filters/formatting. -->
	<div class="behbar row row-wrap" class:panel-open={mobilePanel === 'auto'} role="group" aria-label={m.conversation_behavior_aria()}>
		<Toggle
			pressed={autoApprove}
			style="--toggle-accent: var(--warn)"
			title={m.conversation_auto_approve_title()}
			aria-label={m.conversation_auto_approve_aria()}
			onclick={ontoggleAuto}
		>{m.conversation_auto_approve_btn()}</Toggle>
		{#if ondiagnose}
			<Toggle
				pressed={false}
				title={m.conversation_diagnose_title()}
				onclick={ondiagnose}
			>{m.conversation_diagnose_btn()}</Toggle>
		{/if}
		{#if onterminal}
			<Toggle
				pressed={terminalOpen}
				title={m.conversation_terminal_title()}
				onclick={onterminal}
			>{m.conversation_terminal_btn()}</Toggle>
		{/if}
	</div>
</div>

<style>
	/* The trigger reads like the Toggles beside it. `.bare` in the selector:
	   the kit's `.pop-trigger.bare` ties on specificity and loads later. */
	:global(.filters-trigger.bare) {
		display: inline-flex;
		align-items: center;
		padding: 0.15rem var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-pill);
		background: var(--bg-elevated-2);
		color: var(--text-muted);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		line-height: 1.4;
		white-space: nowrap;
	}

	/* Toolbar: three visually-separated groups — message-category
	   filter, formatting toggles, behavior toggle — divided by thin rules. */
	.toolbar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
		overflow-x: auto;
		font-size: var(--fs-xs);
		/* This bar hosts a UI-scale slider, and the app scales by
		   changing the ROOT font-size — so every rem here (button font-size +
		   horizontal padding) grew while dragging, widening the buttons left of
		   the slider and shoving the slider out from under the cursor → the same
		   "seizure" the header had. Pin the bar's size tokens to px (the exact
		   rem values at the 16px base) so the toolbar's geometry is scale-immune:
		   the chat messages in `.conv` still rescale live, the slider's row does
		   not move. Mirrors the `.hd` fix in Header.svelte. */
		--fs-xs: 12px;
		--fs-sm: 13px;
		--sp-1: 4px;
		--sp-2: 8px;
		--sp-3: 12px;
	}
	.tagbar,
	.fmtbar,
	.behbar {
		gap: var(--sp-1);
	}
	.fmtbar,
	.behbar {
		padding-left: var(--sp-3);
		border-left: 1px solid var(--border);
	}
	/* The filter chips, formatting + behavior toggles, and mobile tabs are all
	   <Toggle> chips now; their base/on-state styling lives in Toggle.svelte.
	   Per-use tint (role color / warm auto-approve) is set via --toggle-accent
	   inline. Only the mobile tabs need a layout override (full-width, larger). */
	/* Mobile-tab triggers: hidden on desktop where the groups inline. */
	.mobile-tabs {
		display: none;
	}
	.mobile-tabs :global(.toggle.mtab) {
		flex: 1;
		padding: 0.3rem var(--sp-2);
		font-size: var(--fs-sm);
	}
	@media (max-width: 959px) {
		.toolbar {
			position: relative;
			/* The popovers float above the message log; keep the bar itself a single
			   tidy row of triggers and let panels overlay rather than push content. */
			overflow: visible;
		}
		.mobile-tabs {
			display: flex;
			gap: var(--sp-2);
			width: 100%;
		}
		/* Collapse the inline groups; each reappears as an absolute popover when
		   its trigger is active. */
		.tagbar,
		.fmtbar,
		.behbar {
			display: none;
		}
		.tagbar.panel-open,
		.fmtbar.panel-open,
		.behbar.panel-open {
			display: flex;
			position: absolute;
			top: calc(100% + var(--sp-1));
			left: 0;
			right: 0;
			z-index: 5;
			padding: var(--sp-2);
			/* Drop the desktop divider that separated fmt/beh from the filters. */
			padding-left: var(--sp-2);
			border-left: none;
			background: var(--bg-elevated-2);
			border: 1px solid var(--border-strong);
			border-radius: var(--r-md);
			box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
		}
	}
</style>
