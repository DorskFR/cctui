<script lang="ts">
	// The drawer is now only the side panel: everything inside it is
	// ConversationPane, which the tiles view mounts without a panel around it.
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { ResizablePanel } from '@dorsk/tsumikit';
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
		{/snippet}
	</ResizablePanel>
</div>

<style>
	/* Zero-size stacking-context host so the fixed panel and scrim paint on
	   the drawer layer instead of inside the page's own stacking order. */
	.drawer-host {
		position: relative;
		z-index: var(--z-drawer);
	}
</style>
