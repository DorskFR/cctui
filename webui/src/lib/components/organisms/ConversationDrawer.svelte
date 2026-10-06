<script lang="ts">
	// The drawer is now only the side panel: everything inside it is
	// ConversationPane, which the tiles view mounts without a panel around it.
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Button, ResizablePanel, Text } from '@dorsk/tsumikit';
	import ConversationPane from './ConversationPane.svelte';
	import { drafts, VIEW_OPTS } from '$lib/drafts';
	import { parseViewOpts } from './conversation/filters';
	import { lockDocumentScroll } from './conversation/scrollLock';
	import { m } from '$lib/paraglide/messages';

	let {
		session,
		onclose,
		highlight = [],
		focusSeq = null,
		onNewFromScript,
		onFollowup,
		onNavigate
	}: {
		session: SessionListItem;
		onclose: () => void;
		highlight?: string[];
		/** Causal seq of the matched message when opened from a search hit
		 *  (`SessionListItem.match_seq`). `null` opens tail-anchored as usual. */
		focusSeq?: number | null;
		// "New session from same script" for archived sessions.
		onNewFromScript?: (s: SessionListItem) => void;
		onFollowup?: (s: SessionListItem, instruction?: string) => void;
		// Open another session in place by id — used to jump straight to a
		// freshly forked conversation without a manual refresh.
		onNavigate?: (sessionId: string) => void;
	} = $props();

	$effect(() => lockDocumentScroll());

	const DRAWER_MIN_PX = 360;
	const DRAWER_DEFAULT_PX = 900;
	// Only the persisted pane width is read here; the pane owns the rest of the
	// view options.
	const paneWidth = parseViewOpts(drafts.get(VIEW_OPTS)).paneWidth;
</script>

<div class="drawer-host">
	<ResizablePanel
		mode="overlay"
		side="right"
		open
		{onclose}
		label={m.settings_group_conversation()}
		width={paneWidth ?? DRAWER_DEFAULT_PX}
		minWidth={DRAWER_MIN_PX}
		maxWidth="100vw"
		widthKey="cctui_drawer_width"
		fullWidthBelow="959px"
		handlePlacement="top"
		resizeStep={32}
	>
		{#snippet panel()}
			<svelte:boundary onerror={(e) => console.error('conversation drawer crashed', e)}>
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
		{/snippet}
	</ResizablePanel>
</div>

<style>
	.crashed {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--sp-3);
		padding: var(--sp-4);
		padding-top: calc(var(--sp-4) + var(--safe-top));
	}
	.crash-message {
		margin: 0;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		font-size: var(--fs-xs);
		color: var(--text-muted);
	}
	/* Zero-size stacking-context host so the fixed panel and scrim paint on
	   the drawer layer instead of inside the page's own stacking order. */
	.drawer-host {
		position: relative;
		z-index: var(--z-drawer);
	}
</style>
