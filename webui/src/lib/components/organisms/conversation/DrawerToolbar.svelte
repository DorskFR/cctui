<script lang="ts">
	// Message filters on the left; auto-approve, the panels menu, context and
	// pins right-aligned. Narrow bars drop the optional labels and wrap.
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
	import { Icon, Menu, Popover, Switch, Toggle } from '@dorsk/tsumikit';
	import { panelItems } from './toolbarPanels';
	import { ComposerFocus } from './composerFocus.svelte';
	import PinsPanel from './PinsPanel.svelte';
	import SessionContextChip from './SessionContextChip.svelte';
	import type { MessagePin } from '@bindings/MessagePin';
	import type { Line } from './types';
	import type { PluginButton } from '#lib/plugins/types.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { hasFeature } from '#lib/features.svelte.ts';

	let {
		view = $bindable(),
		autoApprove,
		ontoggleAuto,
		terminalOpen = false,
		ontoggleTerminal,
		plugins = [],
		pins = [],
		lines = [],
		onjumpseq,
		onunpin,
		sessionId,
		contextAttachable = false
	}: {
		view: ViewOpts;
		autoApprove: boolean;
		ontoggleAuto: () => void;
		/** Read-only live terminal pane; omit the handler to hide the toggle. */
		terminalOpen?: boolean;
		ontoggleTerminal?: () => void;
		/** Enabled plugin panes; one toggle each, pressed while its pane is open. */
		plugins?: PluginButton[];
		pins?: MessagePin[];
		lines?: Line[];
		/** Omit both to hide the pins button (e.g. no session context). */
		onjumpseq?: (seq: number) => void;
		onunpin?: (seq: number) => void;
		/** Omit to hide the session context chip. */
		sessionId?: string;
		contextAttachable?: boolean;
	} = $props();

	// One pinned height for every control, so glyph fonts can't size them.
	const CTL = 'height:var(--bar-ctl-h);box-sizing:border-box;padding-block:0;line-height:1';

	let hostEl = $state<HTMLElement>();
	const composer = new ComposerFocus();
	$effect(() => (hostEl ? composer.attach(hostEl) : undefined));

	const panels = $derived(
		panelItems(
			ontoggleTerminal && hasFeature('terminal') ? { open: terminalOpen, toggle: ontoggleTerminal } : null,
			hasFeature('plugins') ? plugins : []
		)
	);

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
</script>

<div class="tbhost" class:typing={composer.collapse} bind:this={hostEl}>
<div class="toolbar">
	<!-- A quick chip is on only when every category it covers is on; a partly-on
	     group gets a dashed border. -->
	<div class="tagbar row" data-journey="filters" role="group" aria-label={m.conversation_msg_filter_aria()}>
		{#each QUICK_FILTERS as q (q.id)}
			<Toggle
				pill
				data-journey="quick"
				data-journey-key={q.id}
				pressed={quickOn(view.msgFilter, q.id)}
				style={`${CTL};--accent: ${QUICK_TINT[q.id]}${quickPartial(view.msgFilter, q.id) ? ';border-style:dashed' : ''}`}
				title={quickTitle(q.id)}
				onclick={() => toggleQuick(q.id)}
			>
				{#if quickOn(view.msgFilter, q.id)}<Icon name="check" size={12} />{/if}
				{quickFilterLabel(q.id)}
			</Toggle>
		{/each}
		<Popover
			label={m.conversation_filter_menu_aria()}
			placement="bottom-start"
			variant="toggle"
			pill
			style={CTL}
			data-journey="filter-menu"
		>
			{#snippet trigger()}
				<Icon name="filter" size={12} />
				<span class="wide"
					>{offCount > 0
						? m.conversation_filters_off_count({ count: offCount })
						: m.conversation_filters()}</span
				>{#if offCount > 0}<span class="narrow">{offCount}</span>{/if}
			{/snippet}
			<FilterMenu
				filter={view.msgFilter}
				ontoggle={toggleCategory}
				onall={(on) => (view.msgFilter = allFilter(on))}
			/>
		</Popover>
	</div>
	<div class="behbar row" role="group" aria-label={m.conversation_behavior_aria()}>
		{#if hasFeature('auto_approve')}
			<span class="auto" title={m.conversation_auto_approve_title()}>
				<Switch
					size="sm"
					labelVisible
					label={autoApprove ? m.conversation_auto_approve_on() : m.conversation_auto_approve_off()}
					data-journey="auto-approve"
					bind:checked={() => autoApprove, () => ontoggleAuto()}
				/>
			</span>
		{/if}
		{#if panels.length}
			<Menu
				label={m.conversation_panels()}
				items={panels}
				placement="bottom-end"
				variant="toggle"
				pressed={panels.some((i) => i.pressed)}
				style={CTL}
				data-journey="panels"
			>
				{#snippet trigger()}
					<Icon name="layout-grid" size={12} /><span class="wide">{m.conversation_panels()}</span>
				{/snippet}
			</Menu>
		{/if}
		{#if sessionId && hasFeature('context')}
			<SessionContextChip {sessionId} canAttach={contextAttachable} style={CTL} />
		{/if}
		{#if onjumpseq && onunpin}
			<Popover
				label={m.conversation_pins_aria()}
				placement="bottom-end"
				variant="toggle"
				style={CTL}
			>
				{#snippet trigger()}
					<Icon name="pin" size={12} filled={pins.length > 0} />
					<span class="wide">{m.conversation_pins()}</span>{pins.length ? ` ${pins.length}` : ''}
				{/snippet}
				<PinsPanel {pins} {lines} onjump={onjumpseq} {onunpin} />
			</Popover>
		{/if}
	</div>
</div>
</div>

<style>
	.tbhost {
		container: drawer-toolbar / inline-size;
	}
	.tbhost.typing {
		display: none;
	}
	.toolbar {
		display: flex;
		flex-wrap: nowrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg-elevated);
		font-size: var(--fs-xs);
		/* Px, not rem: the text-size control must not shift the bar under the cursor. */
		--bar-ctl-h: 24px;
		--fs-xs: 12px;
		--fs-sm: 13px;
		--sp-1: 4px;
		--sp-2: 8px;
		--sp-3: 12px;
	}
	.tagbar,
	.behbar {
		gap: var(--sp-1);
		flex-wrap: nowrap;
		align-items: center;
	}
	.auto {
		display: inline-flex;
		align-items: center;
		height: var(--bar-ctl-h);
		white-space: nowrap;
	}
	/* Overflow is absorbed by the filters group, never by auto-approve or pins. */
	.tagbar {
		min-width: 0;
		flex: 0 1 auto;
		overflow: hidden;
	}
	.behbar {
		flex: none;
		margin-left: auto;
	}
	.narrow {
		display: none;
	}
	/* The full form needs ~940px in the longest locale. */
	@container drawer-toolbar (max-width: 1000px) {
		.wide {
			display: none;
		}
		.narrow {
			display: inline;
		}
	}
	/* A phone wraps the behaviour group under the filters instead of clipping it. */
	@container drawer-toolbar (max-width: 32rem) {
		.toolbar {
			flex-wrap: wrap;
		}
	}
</style>
