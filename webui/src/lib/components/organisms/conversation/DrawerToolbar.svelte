<script lang="ts">
	// Conversation toolbar, one row at every width: message filters on the left,
	// auto-approve and pins on the right. Narrow drawers drop the labels.
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
	import { Icon, Popover, Toggle } from '@dorsk/tsumikit';
	import PinsPanel from './PinsPanel.svelte';
	import type { MessagePin } from '@bindings/MessagePin';
	import type { Line } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		view = $bindable(),
		autoApprove,
		ontoggleAuto,
		pins = [],
		lines = [],
		onjumpseq,
		onunpin,
		hitCount = 0,
		hitIndex = -1,
		onprevhit,
		onnexthit
	}: {
		view: ViewOpts;
		autoApprove: boolean;
		ontoggleAuto: () => void;
		pins?: MessagePin[];
		lines?: Line[];
		/** Omit both to hide the pins button (e.g. no session context). */
		onjumpseq?: (seq: number) => void;
		onunpin?: (seq: number) => void;
		/** Search-hit stepping; the whole group hides when there are no hits. */
		hitCount?: number;
		hitIndex?: number;
		onprevhit?: () => void;
		onnexthit?: () => void;
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
</script>

<div class="toolbar">
	<!-- Quick category toggles + the full per-category picker. A quick chip is
	     "on" only when every category it covers is on; a partly-on group gets a
	     dashed border instead. -->
	<div class="tagbar row" data-journey="filters" role="group" aria-label={m.conversation_msg_filter_aria()}>
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
		<Popover
			label={m.conversation_filter_menu_aria()}
			placement="bottom-start"
			bare
			triggerClass="toolbar-chip"
		>
			{#snippet trigger()}
				<span class="chip pill" data-journey="filter-menu">
					<Icon name="filter" size={12} />
					<span class="wide"
						>{offCount > 0
							? m.conversation_filters_off_count({ count: offCount })
							: m.conversation_filters()}</span
					>{#if offCount > 0}<span class="narrow">{offCount}</span>{/if}
				</span>
			{/snippet}
			<FilterMenu
				filter={view.msgFilter}
				ontoggle={toggleCategory}
				onall={(on) => (view.msgFilter = allFilter(on))}
			/>
		</Popover>
	</div>
	{#if hitCount > 0}
		<div class="hitbar row" role="group" aria-label={m.conversation_hits_aria()}>
			<Toggle pressed={false} title={m.conversation_hit_prev()} onclick={onprevhit}>↑</Toggle>
			<Toggle pressed={false} title={m.conversation_hit_next()} onclick={onnexthit}>↓</Toggle>
			<span class="hit-count" aria-live="polite"
				>{m.conversation_hit_counter({ n: hitIndex + 1, total: hitCount })}</span
			>
		</div>
	{/if}
	<div class="behbar row" role="group" aria-label={m.conversation_behavior_aria()}>
		<Toggle
			pressed={autoApprove}
			style="--toggle-accent: var(--warn)"
			title={m.conversation_auto_approve_title()}
			aria-label={m.conversation_auto_approve_aria()}
			onclick={ontoggleAuto}
			><span aria-hidden="true">⚡</span><span class="wide"> {m.conversation_auto_approve_btn()}</span></Toggle
		>
		{#if onjumpseq && onunpin}
			<Popover
				label={m.conversation_pins_aria()}
				placement="bottom-end"
				bare
				triggerClass="toolbar-chip"
			>
				{#snippet trigger()}
					<span class="chip">
						<Icon name="pin" filled={pins.length > 0} />
						<span class="wide">{m.conversation_pins()}</span>{pins.length ? ` ${pins.length}` : ''}
					</span>
				{/snippet}
				<PinsPanel {pins} {lines} onjump={onjumpseq} {onunpin} />
			</Popover>
		{/if}
	</div>
</div>

<style>
	/* The Filters and Pins popover triggers must be indistinguishable from the
	   Toggle chips beside them. `bare` strips the kit's own trigger chrome (its
	   square floor included) and this local span — authored here, so scoped CSS
	   reaches it — carries the whole chip restated from Toggle's own tokens.
	   Keep in step with tsumikit Toggle's `.toggle` + `.pill`. */
	.chip {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 4px;
		padding: 0.15rem var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
		color: var(--text-muted);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		line-height: 1.4;
		white-space: nowrap;
		user-select: none;
		cursor: pointer;
		transition:
			background 0.12s var(--ease),
			border-color 0.12s var(--ease),
			color 0.12s var(--ease);
	}
	/* Filters sits among the pill quick-filter chips; Pins among the square ones. */
	.chip.pill {
		border-radius: var(--r-pill);
	}
	.chip:hover {
		border-color: var(--border-strong);
	}

	.toolbar {
		display: flex;
		flex-wrap: nowrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
		font-size: var(--fs-xs);
		/* Px, not rem: the text-size control rescales the root, and the bar must
		   not shift under the cursor while it moves. */
		--fs-xs: 12px;
		--fs-sm: 13px;
		--sp-1: 4px;
		--sp-2: 8px;
		--sp-3: 12px;
	}
	.tagbar,
	.behbar,
	.hitbar {
		gap: var(--sp-1);
		flex-wrap: nowrap;
	}
	.hitbar {
		align-items: center;
	}
	.hit-count {
		color: var(--text-muted);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
	.behbar,
	.hitbar {
		padding-left: var(--sp-3);
		border-left: 1px solid var(--border);
	}
	.narrow {
		display: none;
	}
	@media (max-width: 959px) {
		.wide {
			display: none;
		}
		.narrow {
			display: inline;
		}
		.behbar {
			margin-left: auto;
			padding-left: 0;
			border-left: none;
		}
	}
</style>
