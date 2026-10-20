<script lang="ts">
	// The conversation pinned to one edge of the Sessions screen (Settings ›
	// Sessions › Always-visible conversation panel), in place of the overlay
	// drawer. The list stays clickable beside it: picking another row swaps the
	// pane in place. `width` is whatever the layout reserved on this edge
	// (resolveDocks), so the two never drift apart; the grip on the inner edge
	// writes a new width back to the settings. With nothing open it keeps its
	// column and says so, rather than collapsing and reflowing the list.
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Button, Text, resizeHandle } from '@dorsk/tsumikit';
	import ConversationPane from './ConversationPane.svelte';
	import { DOCK_MIN_PX, maxDockWidth, type DockSide } from '$lib/dock';
	import { settings } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		side,
		width,
		session,
		onclose,
		highlight = [],
		focusSeq = null,
		onNewFromScript,
		onFollowup,
		onNavigate
	}: {
		side: DockSide;
		width: string;
		/** The open session, or `null` for the empty column. */
		session: SessionListItem | null;
		onclose: () => void;
		highlight?: string[];
		focusSeq?: number | null;
		onNewFromScript?: (s: SessionListItem) => void;
		onFollowup?: (s: SessionListItem, instruction?: string) => void;
		onNavigate?: (sessionId: string) => void;
	} = $props();

	let dragging = $state(false);
	let viewportWidth = $state(0);
	const maxPx = $derived(maxDockWidth(viewportWidth));
</script>

<svelte:window bind:innerWidth={viewportWidth} />

<aside
	class="dock"
	class:dock-left={side === 'left'}
	style:--conv-dock-w={width}
	aria-label={m.settings_group_conversation()}
	data-journey="conversation-dock"
>
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<div
		class="grip"
		class:grip-left={side === 'left'}
		class:dragging
		role="separator"
		tabindex="0"
		aria-orientation="vertical"
		aria-valuemin={DOCK_MIN_PX}
		aria-valuemax={maxPx}
		aria-label={m.dock_resize_grip()}
		title={m.dock_resize_grip()}
		use:resizeHandle={{
			side,
			min: DOCK_MIN_PX,
			max: maxPx,
			onwidth: (px) => settings.setConversationDock({ width: px }),
			onreset: () => settings.setConversationDock({ width: undefined }),
			onactive: (a) => {
				dragging = a;
				document.body.classList.toggle('dock-resizing', a);
			}
		}}
	></div>
	{#if session}
		<!-- One pane per session: switching rows remounts it, so per-pane state
		     (terminal, plugin pane, scroll anchor) never leaks to the next one. -->
		{#key session.id}
			<svelte:boundary onerror={(e) => console.error('conversation dock crashed', e)}>
				<ConversationPane
					chrome="drawer"
					{session}
					{onclose}
					{highlight}
					{focusSeq}
					{onNewFromScript}
					{onFollowup}
					{onNavigate}
				/>
				{#snippet failed(error, reset)}
					<div class="crashed" role="alert">
						<Text weight="medium">{m.conversation_crashed()}</Text>
						<pre class="crash-message">{error instanceof Error ? error.message : String(error)}</pre>
						<Button onclick={reset}>{m.common_retry()}</Button>
					</div>
				{/snippet}
			</svelte:boundary>
		{/key}
	{:else}
		<div class="empty">
			<Text tone="muted">{m.conversation_dock_empty()}</Text>
		</div>
	{/if}
</aside>

<style>
	.dock {
		position: fixed;
		top: calc(var(--header-h) + var(--safe-top));
		bottom: var(--bottom-chrome, calc(var(--nav-h) + var(--safe-bottom)));
		right: 0;
		width: var(--conv-dock-w);
		display: flex;
		flex-direction: column;
		background: var(--bg);
		border-left: 1px solid var(--border);
		z-index: 4;
	}
	.dock.dock-left {
		right: auto;
		left: 0;
		border-left: 0;
		border-right: 1px solid var(--border);
	}
	/* A 10px hit area straddling the panel's border, with a 2px line that only
	   shows on hover, focus or while dragging so the border stays quiet otherwise. */
	.grip {
		position: absolute;
		top: 0;
		bottom: 0;
		left: -5px;
		width: 10px;
		cursor: ew-resize;
		touch-action: none;
		z-index: 1;
	}
	.grip-left {
		left: auto;
		right: -5px;
	}
	/* An always-visible knob in the middle of the edge, like the kit's panel handle. */
	.grip::before {
		content: '';
		position: absolute;
		top: 50%;
		left: 3px;
		width: 4px;
		height: 2.5rem;
		margin-top: -1.25rem;
		border-radius: var(--r-pill);
		background: var(--border-strong);
	}
	.grip:hover::before,
	.grip.dragging::before {
		background: var(--accent);
	}
	.grip::after {
		content: '';
		position: absolute;
		top: 0;
		bottom: 0;
		left: 4px;
		width: 2px;
		background: var(--accent);
		opacity: 0;
		transition: opacity 0.12s var(--ease);
	}
	.grip:hover::after,
	.grip:focus-visible::after,
	.grip.dragging::after {
		opacity: 1;
	}
	.grip:focus-visible {
		outline: none;
	}
	@media (hover: none) {
		.grip::after {
			opacity: 0.35;
		}
	}
	.empty {
		flex: 1;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: var(--sp-4);
		text-align: center;
	}
	.crashed {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--sp-3);
		padding: var(--sp-4);
	}
	.crash-message {
		margin: 0;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		font-size: var(--fs-xs);
		color: var(--text-muted);
	}
</style>
