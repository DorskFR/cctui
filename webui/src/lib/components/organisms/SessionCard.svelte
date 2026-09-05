<script lang="ts">
	import type { Label } from '@bindings/Label';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { m } from '$lib/paraglide/messages';
	import { Card } from '@dorsk/tsumikit';
	import { onMount } from 'svelte';
	import CompactRow from './sessioncard/CompactRow.svelte';
	import DetailedCard from './sessioncard/DetailedCard.svelte';
	import { SwipeGesture } from './sessioncard/swipe.svelte';
	import { type SessionActions, type SubagentToggle, buildView } from './sessioncard/view';

	// Two layouts only: the compact list row and the detailed card. The wrapper
	// is the `sess-card` size container every readout degrades against.
	let {
		session,
		variant = 'card',
		child = false,
		showMachine = true,
		pendingCount = 0,
		unreadCount = 0,
		onopen,
		selectable = false,
		selected = false,
		onToggleSelect,
		swipeable = false,
		swipeLabel = m.sessions_archive(),
		onSwipe,
		onTogglePin,
		highlight = [],
		subagentCost = null,
		subagentToggles = [],
		stacked = false,
		allLabels = [],
		onCreateLabel,
		onAttachLabel,
		onDetachLabel,
		onUpdateLabel,
		onDeleteLabel,
		draft = false,
		draftLaunching = false,
		preview = null,
		onLaunch,
		onEdit,
		onDiscard,
		accentHue = null
	}: {
		session: SessionListItem;
		variant?: 'row' | 'card';
		child?: boolean;
		/** Off when the section header already names the machine. */
		showMachine?: boolean;
		pendingCount?: number;
		/** Unread assistant messages; the caller passes 0 for the open session. */
		unreadCount?: number;
		onopen: (s: SessionListItem) => void;
		highlight?: string[];
		selectable?: boolean;
		selected?: boolean;
		/** `range` is true when Shift was held. */
		onToggleSelect?: (s: SessionListItem, range?: boolean) => void;
		swipeable?: boolean;
		swipeLabel?: string;
		onSwipe?: (s: SessionListItem) => void;
		onTogglePin?: (s: SessionListItem) => void;
		/** Parent's own tokens plus every subagent's, with the subagent count. */
		subagentCost?: { tokens: number; count: number } | null;
		subagentToggles?: SubagentToggle[];
		stacked?: boolean;
		allLabels?: Label[];
		onCreateLabel?: (name: string, color: string) => Promise<Label>;
		onAttachLabel?: (id: string, labelId: string) => void | Promise<void>;
		onDetachLabel?: (id: string, labelId: string) => void | Promise<void>;
		onUpdateLabel?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDeleteLabel?: (labelId: string) => void | Promise<void>;
		/** Staged spawn: inert body, Launch/Edit/Discard in the trailing slot. */
		draft?: boolean;
		draftLaunching?: boolean;
		/** Drafts show their staged prompt here (no last message yet). */
		preview?: string | null;
		onLaunch?: (s: SessionListItem) => void;
		onEdit?: (s: SessionListItem) => void;
		onDiscard?: (s: SessionListItem) => void;
		/** Color-by hue for the left strip; null = none. */
		accentHue?: number | null;
	} = $props();

	const row = $derived(variant === 'row');

	// 5s tick: the tool cadence needs second-ish freshness; the 30-min stale
	// signal rides the same clock.
	let now = $state(Date.now());
	onMount(() => {
		const t = setInterval(() => (now = Date.now()), 5_000);
		return () => clearInterval(t);
	});

	const view = $derived(
		buildView(session, {
			child,
			showMachine,
			now,
			preview,
			highlight,
			subagentCost,
			pendingCount,
			unreadCount,
			draft,
			draftLaunching
		})
	);
	const actions = $derived<SessionActions>({
		selectable,
		selected,
		subagentToggles,
		onTogglePin,
		labelEditable: !!onAttachLabel && !child,
		allLabels,
		onCreateLabel,
		onAttachLabel,
		onDetachLabel,
		onUpdateLabel,
		onDeleteLabel,
		onLaunch,
		onEdit,
		onDiscard
	});

	const swipe = new SwipeGesture(
		() => swipeable && !selectable,
		() => onSwipe?.(session)
	);

	// Surface state lives inline on the Card: scoped CSS can't reach a child
	// component's root. The transform is only set mid-swipe — at rest it would
	// trap the stacked pseudo-elements' z-index inside the card. The attention
	// fill is opaque so a stacked back-card never bleeds through.
	const cardStyle = $derived(
		[
			swipe.x !== 0 ? `transform: translateX(${swipe.x}px)` : '',
			`transition: ${swipe.active ? 'none' : 'transform 0.2s var(--ease)'}`,
			view.needsInput
				? 'background: var(--attention-bg-solid); border-left: 3px solid var(--attention-bar)'
				: '',
			child && !view.needsInput
				? 'border-color: color-mix(in srgb, var(--info) 45%, var(--border))'
				: '',
			accentHue != null && !view.needsInput
				? `--mh:${accentHue}; background: color-mix(in srgb, hsl(var(--mh) 65% 50%) 8%, var(--bg-elevated)); border-left: 3px solid hsl(var(--mh) var(--mach-border-sl))`
				: '',
			selected
				? 'background: color-mix(in srgb, var(--accent) 12%, var(--bg-elevated)); box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 55%, transparent)'
				: ''
		]
			.filter(Boolean)
			.join('; ')
	);

	function handleClick(e?: MouseEvent | KeyboardEvent) {
		if (e?.target instanceof Element && e.target.closest('[popovertarget],[popover]')) return;
		if (draft) return;
		if (swipe.consumeClick()) return;
		if (selectable) {
			if (e?.shiftKey) window.getSelection()?.removeAllRanges();
			onToggleSelect?.(session, e?.shiftKey === true);
		} else onopen(session);
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="sc-wrap"
	class:stale={view.stale}
	class:child
	class:compact={row}
	onpointerdown={swipe.start}
	onpointermove={swipe.move}
	onpointerup={swipe.end}
	onpointercancel={swipe.end}
>
	{#if swipeable && swipe.x < 0}
		<div class="swipe-reveal" style="opacity: {0.25 + 0.75 * swipe.progress}" aria-hidden="true">
			<div class="swipe-reveal-inner" class:armed={swipe.progress >= 1}>
				<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<rect x="3" y="4" width="18" height="4" rx="1" />
					<path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8" />
					<path d="M9 12h6" />
				</svg>
				<span>{swipe.progress >= 1 ? m.sessions_swipe_release({ action: swipeLabel.toLowerCase() }) : swipeLabel}</span>
			</div>
		</div>
	{/if}
	<!-- A <div> Card, not a <button>: the row hosts its own controls (label
	     picker, star, toggles) which can't nest inside a button. -->
	<Card
		as="div"
		interactive
		{stacked}
		stackTone="info"
		padding={row ? 'sm' : 'md'}
		style={cardStyle}
		data-session-id={session.id}
		onclick={handleClick}
	>
		{#if row}
			<CompactRow {view} {actions} />
		{:else}
			<DetailedCard {view} {actions} />
		{/if}
	</Card>
</div>

<style>
	.sc-wrap {
		position: relative;
		width: 100%;
		height: 100%;
		touch-action: pan-y;
		container: sess-card / inline-size;
	}
	/* Rows and cards degrade at different widths, so each names its own container. */
	.sc-wrap.compact {
		container: sess-row / inline-size;
	}
	.sc-wrap.child {
		width: auto;
		margin-left: var(--sp-4);
	}
	.sc-wrap.compact.child {
		margin-left: 14px;
	}
	.sc-wrap.stale {
		opacity: 0.6;
	}
	.swipe-reveal {
		position: absolute;
		inset: 0;
		z-index: 0;
		display: flex;
		align-items: center;
		justify-content: flex-end;
		padding-right: var(--sp-4);
		border-radius: var(--r-md);
		background: color-mix(in srgb, var(--warn) 22%, var(--bg));
		color: var(--warn);
	}
	.swipe-reveal-inner {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
		transition: transform 0.12s var(--ease);
	}
	.swipe-reveal-inner.armed {
		transform: scale(1.12);
	}
</style>
