<script lang="ts">
	// Message filters on the left; auto-approve, side panels, context and pins
	// right-aligned. Narrow bars drop the secondary labels, never the state words.
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
	import { Icon, Menu, Popover, Switch, Toggle, type MenuItem } from '@dorsk/tsumikit';
	import PinsPanel from './PinsPanel.svelte';
	import SessionContextChip from './SessionContextChip.svelte';
	import type { MessagePin } from '@bindings/MessagePin';
	import type { Line } from './types';
	import type { PluginButton } from '$lib/plugins/types';
	import { m } from '$lib/paraglide/messages';

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
		/** Read-only live terminal pane; omit the handler to hide the entry. */
		terminalOpen?: boolean;
		ontoggleTerminal?: () => void;
		/** Enabled plugin panes; one entry each, checked while its pane is open. */
		plugins?: PluginButton[];
		pins?: MessagePin[];
		lines?: Line[];
		/** Omit both to hide the pins button. */
		onjumpseq?: (seq: number) => void;
		onunpin?: (seq: number) => void;
		/** Omit to hide the session context chip. */
		sessionId?: string;
		contextAttachable?: boolean;
	} = $props();

	// One pinned height for every control, so glyph fonts can't size them.
	const CTL = 'height:var(--bar-ctl-h);box-sizing:border-box;padding-block:0;line-height:1';
	// One selected look for every filter: neutral fill and a check, never a role colour.
	const PILL = `${CTL};--toggle-accent: var(--text)`;

	const offCount = $derived(MSG_CATEGORIES.filter((c) => !view.msgFilter[c]).length);

	const panelItems = $derived<MenuItem[]>([
		...(ontoggleTerminal
			? [
					{
						label: m.drawer_terminal_label(),
						icon: 'tv' as const,
						pressed: terminalOpen,
						keepOpen: true,
						attrs: { title: m.conversation_terminal_title(), 'data-journey': 'terminal' },
						onselect: ontoggleTerminal
					}
				]
			: []),
		...plugins.map((p) => ({
			id: `plugin-${p.id}`,
			label: p.label,
			icon: p.icon,
			pressed: p.open,
			keepOpen: true,
			attrs: {
				title: m.drawer_plugin_toggle({ name: p.label }),
				'data-journey': 'plugin',
				'data-plugin': p.id
			} as MenuItem['attrs'],
			onselect: p.onselect
		}))
	]);
	const panelsOpen = $derived((!!ontoggleTerminal && terminalOpen) || plugins.some((p) => p.open));

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

<div class="toolbar">
	<div class="tbrow">
		<!-- A quick chip is "on" only when every category it covers is on; a
		     partly-on group gets a dashed border instead. -->
		<div class="tagbar row" data-journey="filters" role="group" aria-label={m.conversation_msg_filter_aria()}>
			{#each QUICK_FILTERS as q (q.id)}
				{@const on = quickOn(view.msgFilter, q.id)}
				<Toggle
					pill
					data-journey="quick"
					data-journey-key={q.id}
					pressed={on}
					style={`${PILL}${quickPartial(view.msgFilter, q.id) ? ';border-style:dashed' : ''}`}
					title={quickTitle(q.id)}
					onclick={() => toggleQuick(q.id)}
				>
					{#if on}<Icon name="check" size={12} />{/if}{quickFilterLabel(q.id)}
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
			<span class="auto" title={m.conversation_auto_approve_title()} data-journey="auto-approve">
				<Switch
					size="sm"
					labelVisible
					label={autoApprove ? m.chrome_auto_approve_on() : m.chrome_auto_approve_off()}
					bind:checked={() => autoApprove, () => ontoggleAuto()}
				/>
			</span>
			{#if panelItems.length > 0}
				<Menu
					label={m.chrome_panels_aria()}
					items={panelItems}
					placement="bottom-end"
					variant="toggle"
					style={CTL}
					data-journey="panels"
				>
					{#snippet trigger()}
						<Icon name="layout-grid" size={12} />{m.chrome_panels()}{#if panelsOpen}<span
								class="dot"
								aria-hidden="true"
							></span>{/if}
					{/snippet}
				</Menu>
			{/if}
			{#if sessionId}
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
	.toolbar {
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
		font-size: var(--fs-xs);
		container: drawer-toolbar / inline-size;
		/* Px, not rem: the text-size control must not shift the bar under the cursor. */
		--bar-ctl-h: 24px;
		--fs-xs: 12px;
		--fs-sm: 13px;
		--sp-1: 4px;
		--sp-2: 8px;
		--sp-3: 12px;
	}
	.tbrow {
		display: flex;
		flex-wrap: nowrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
	}
	.tagbar,
	.behbar {
		gap: var(--sp-1);
		flex-wrap: nowrap;
		align-items: center;
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
		padding-left: var(--sp-3);
		border-left: 1px solid var(--border);
	}
	.auto {
		display: inline-flex;
		align-items: center;
		white-space: nowrap;
	}
	.dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: currentColor;
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
		.behbar {
			padding-left: 0;
			border-left: none;
		}
	}
	/* A phone gets two rows rather than a clipped one. */
	@container drawer-toolbar (max-width: 560px) {
		.tbrow {
			flex-wrap: wrap;
		}
		.tagbar {
			flex: 1 1 100%;
		}
		.behbar {
			margin-left: 0;
		}
	}
</style>
