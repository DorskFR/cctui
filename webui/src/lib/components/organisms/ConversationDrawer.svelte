<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { AgentEvent } from '@bindings/AgentEvent';
	import { ws } from '$lib/ws.svelte';
	import { useConversation, useSessionActions, useLabels, qk } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { drafts, VIEW_OPTS } from '$lib/drafts';
	import { Dropzone, ResizablePanel } from '@dorsk/tsumikit';
	import ForkModal from './conversation/ForkModal.svelte';
	import DrawerHeader from './conversation/DrawerHeader.svelte';
	import DrawerToolbar from './conversation/DrawerToolbar.svelte';
	import ActivityBanner from './conversation/ActivityBanner.svelte';
	import DrawerBanners from './conversation/DrawerBanners.svelte';
	import ForkSelectBar from './conversation/ForkSelectBar.svelte';
	import AutoArchiveNotice from './conversation/AutoArchiveNotice.svelte';
	import TaskPanel from './conversation/TaskPanel.svelte';
	import TerminalPane from './conversation/TerminalPane.svelte';
	import Conversation from './conversation/Conversation.svelte';
	import ConversationComposer from './conversation/ConversationComposer.svelte';
	import PluginPaneHost from './conversation/PluginPaneHost.svelte';
	import { registerComposer } from '$lib/plugins/composerBridge.svelte';
	import { DrawerPlugins } from '$lib/plugins/drawerPlugins.svelte';
	import { usePlugins } from '$lib/queries';
	import { settings } from '$lib/settings.svelte';
	import { scheduledTurns, useScheduledMessages } from '$lib/queries/scheduled';
	import BookmarkSaveModal from './bookmarks/BookmarkSaveModal.svelte';
	import type { ViewOpts } from './conversation/types';
	import { parseViewOpts } from './conversation/filters';
	import { mergeEventSources } from './conversation/format';
	import { ConversationStream, mergeLiveEvent } from './conversation/stream.svelte';
	import { ScrollController } from './conversation/scroll.svelte';
	import { SearchHitStepper } from './conversation/searchHits.svelte';
	import { ForkController } from './conversation/fork.svelte';
	import { ForkSelection } from './conversation/forkSelect.svelte';
	import { SessionActions } from './conversation/sessionActions.svelte';
	import { EarlierPages } from './conversation/earlierPages.svelte';
	import { LineRenderer } from './conversation/lineRender.svelte';
	import { MessagePins } from './conversation/messagePins.svelte';
	import { BookmarkSaver } from './conversation/bookmarkSave.svelte';
	import { guardEscape } from './conversation/escapeGuard';
	import { livenessClass } from './conversation/liveness';
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

	const id = $derived(session.id);
	const archived = $derived(session.status === 'archived');
	const needsInput = $derived(session.attention === 'needs_input' && !archived);
	const showStatusBadge = $derived(session.status === 'new' || session.status === 'archived');
	const qc = useQueryClient();

	// Read-only live terminal pane, toggled from the header menu.
	let terminalOpen = $state(false);
	// Runtime plugins: a pane docked on the drawer's left edge plus the buttons
	// they contribute to assistant lines.
	const installedPlugins = usePlugins();
	const plugins = new DrawerPlugins({
		sessionId: () => id,
		installed: () => installedPlugins.data ?? [],
		enabled: () => settings.pluginsEnabled
	});
	$effect(() => plugins.ensureLoaded());

	const DRAWER_MIN_PX = 360;
	const DRAWER_DEFAULT_PX = 900;
	let view = $state<ViewOpts>(parseViewOpts(drafts.get(VIEW_OPTS)));
	$effect(() => {
		drafts.set(VIEW_OPTS, JSON.stringify(view));
	});

	const history = useConversation(() => id, () => true);
	const actions = useSessionActions();

	// Header labels/star share the session list's label set and mutations so
	// both stay in sync.
	const labelsQuery = useLabels();
	const allLabels = $derived(labelsQuery.data?.labels ?? []);
	const togglePin = (s: SessionListItem) => (s.pinned ? actions.unpin(s.id) : actions.pin(s.id));

	// Shared by the viewport (binds the scroller) and the composer (binds the
	// textarea, whose growth must re-pin the viewport).
	const scroll = new ScrollController();

	const stream = new ConversationStream({
		id: () => id,
		archived: () => archived,
		historyData: () => history.data,
		pin: scroll.stickToBottom,
		invalidateConversation: () => qc.invalidateQueries({ queryKey: qk.conversation(id) }),
		invalidateSessions: () => qc.invalidateQueries({ queryKey: qk.sessionsAll }),
		mergeIntoCache: (sid, ev) =>
			qc.setQueryData<AgentEvent[]>(qk.conversation(sid), (prev) => mergeLiveEvent(prev, ev))
	});
	// (Re)subscribe when the open session changes or a forced resubscribe is
	// requested; tear down listeners on switch/unmount.
	$effect(() => {
		const sid = id;
		void stream.resubTick;
		return stream.subscribe(sid);
	});
	// Catch up after the tab regains focus (the ws may have gone half-open).
	$effect(() => stream.installVisibilityRefresh());

	// Account switcher: opened from the header key glyph or by a soft-limit block.
	let acctModalOpen = $state(false);

	const earlier: EarlierPages = new EarlierPages({
		id: () => id,
		historyLength: () => history.data?.length ?? 0,
		events: () => events
	});
	// History (fetched) + earlier pages + live (ws) events, ordered by causal
	// `seq` (falling back to `ts`); live events already present in history are
	// dropped so a refetch and an optimistic reply's persisted form never
	// render twice.
	const events: AgentEvent[] = $derived(mergeEventSources(history.data ?? [], earlier.pages, stream.live));

	// Opened from a search hit: land centred on it instead of the tail.
	$effect(() => {
		const seq = focusSeq;
		const sid = id;
		if (seq != null) void earlier.focus(sid, seq);
	});
	// `Line` carries no seq, so the focused line is addressed by `ts`. A pruned
	// or filtered-out event resolves to null and the drawer just opens normally.
	const focusTs = $derived.by(() => {
		if (focusSeq == null) return null;
		const ev = events.find((e) => e.seq === focusSeq);
		return ev ? Number(ev.ts) : null;
	});

	const scheduledQuery = useScheduledMessages(() => id);
	const scheduledTurnMap = $derived(scheduledTurns(scheduledQuery.data));
	const renderer = new LineRenderer({
		id: () => id,
		machineId: () => session.machine_id,
		view: () => view,
		highlight: () => highlight,
		events: () => events,
		scheduledTurns: () => scheduledTurnMap,
		pending: () => stream.pendingReplies,
		failed: () => stream.failedReplies,
		retrying: () => stream.retryingReplies,
		askPreamble: () => stream.ask?.preamble,
		planPreamble: () => stream.plan?.preamble
	});
	const lines = $derived(renderer.lines);
	$effect(() => {
		void lines.length;
		void plugins.ready.length;
		plugins.autoOpenFrom(lines);
	});

	// Only follow new content when the user is pinned to the bottom.
	$effect(() => {
		void lines.length;
		void stream.perms.length;
		void stream.working;
		scroll.followIfStuck();
	});
	// Reset to bottom + sticky when switching sessions — except when opened on a
	// search hit, which must land mid-transcript and stay there.
	$effect(() => {
		void id;
		if (focusSeq == null) scroll.resetForSession();
		else scroll.unstick();
	});
	// Keep pinned to the bottom while the composer grows. Re-runs when
	// the scroller / textarea attach (the controller reads both reactively).
	$effect(() => scroll.observeResize());

	const pins = new MessagePins({
		id: () => id,
		events: () => events,
		canFetchOlder: () => earlier.canFetch,
		fetchOlder: earlier.fetchEarlier,
		scroll
	});
	let conv = $state<Conversation>();
	const hits = new SearchHitStepper({
		scroller: () => scroll.scroller,
		loadOlder: () => conv?.loadOlder()
	});
	$effect(() => {
		void lines.length;
		void highlight;
		hits.refresh();
	});
	// A session switch drops the older pages and hit cursor and closes a stale
	// terminal pane.
	$effect(() => {
		void id;
		earlier.reset();
		hits.reset();
		terminalOpen = false;
		plugins.close();
	});

	const isCodexSession = $derived((session.adapter_id ?? '').startsWith('codex'));

	const sa = new SessionActions({
		id: () => id,
		session: () => session,
		events: () => events,
		view: () => view,
		actions,
		onclose: () => onclose()
	});

	const fork = new ForkController({
		id: () => id,
		archived: () => archived,
		isCodex: () => isCodexSession,
		session: () => session,
		fork: (sid, body) => actions.fork(sid, body),
		// Jump straight to the new conversation when claude returned its id;
		// otherwise close and let the list refetch surface it.
		onForked: (sid) => {
			if (sid && onNavigate) onNavigate(sid);
			else onclose();
		}
	});

	// Subset fork from a conversation extract. Claude-only; codex has
	// no partial-fork primitive, so the per-message actions are gated off for it.
	const forkable = $derived(!isCodexSession && !archived);
	const forkSelect = new ForkSelection();
	function forkSelection() {
		const range = forkSelect.range(lines);
		if (range.length === 0) return;
		fork.openExtract({ mode: 'selected', anchor_message_id: null, selected_message_ids: range });
	}

	// Filesystem-backed adapters only; the composer owns the attachment state
	// and the dropzone feeds it via the component ref.
	const supportsAttachments = $derived(
		session.adapter_id === 'claude-code' || session.adapter_id === 'codex'
	);
	let composer = $state<ConversationComposer>();
	$effect(() =>
		registerComposer(id, {
			insertText: (text) => void composer?.insertText(text),
			addFiles: (files) => composer?.addFiles(files),
			focus: () => composer?.focus()
		})
	);

	// Edit a still-pending message: drop the in-flight echo and pull its
	// text back into the composer to fix and resend.
	function editPending(text: string, ts: number) {
		if (archived) return;
		stream.discardOptimistic(ts);
		composer?.loadDraft(text);
	}

	const bookmarks = new BookmarkSaver({ id: () => id, sessionName: () => session.name ?? null });

	function followup(instruction?: string) {
		onFollowup?.(session, instruction);
	}

</script>

<div class="drawer-host">
<ResizablePanel
	mode="overlay"
	side="right"
	open
	{onclose}
	label={m.settings_group_conversation()}
	width={view.paneWidth ?? DRAWER_DEFAULT_PX}
	minWidth={DRAWER_MIN_PX}
	maxWidth="100vw"
	widthKey="cctui_drawer_width"
	fullWidthBelow="959px"
	handlePlacement="top"
	resizeStep={32}
>
	{#snippet panel()}
		{#if plugins.current && plugins.open}
			<PluginPaneHost
				plugin={plugins.current.info}
				module={plugins.current.module}
				{session}
				params={plugins.open.params}
				onclose={() => plugins.close()}
			/>
		{/if}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="drawer"
			class:plugin-open={plugins.current !== null}
			data-journey="conversation"
			onkeydown={guardEscape}
		>
			<!-- The whole drawer is a file drop area: dragging files over it
			     shows the tsumikit Dropzone overlay; on drop they're staged as composer
			     attachments. overlay mode wraps the content without hijacking clicks. -->
			<Dropzone
				overlay
				multiple
				label={m.composer_drop_files()}
				disabled={!supportsAttachments || archived}
				onfiles={(f) => composer?.addFiles(f)}
				onactive={(a) => composer?.setDragActive(a)}
			>
				<DrawerHeader
				{session}
				{archived}
				{isCodexSession}
				livenessClass={livenessClass(session)}
				{showStatusBadge}
				{onclose}
				onrename={sa.rename}
				onsetmodel={sa.setModel}
				oncopylink={sa.copyLink}
				oncopymarkdown={sa.copyMarkdown}
				onexport={sa.export}
				onfork={fork.openDialog}
				onfollowup={onFollowup ? () => followup() : undefined}
				onforkselect={forkable ? forkSelect.toggleMode : undefined}
				forkSelectActive={forkSelect.active}
				onterminal={() => (terminalOpen = !terminalOpen)}
				{terminalOpen}
				plugins={plugins.buttons}
				oninterrupt={sa.interrupt}
				onarchive={sa.archive}
				onstoparchive={sa.stopAndArchive}
				onTogglePin={togglePin}
				onAccountClick={() => (acctModalOpen = true)}
				{allLabels}
				onCreateLabel={actions.createLabel}
				onAttachLabel={actions.attachLabel}
				onDetachLabel={actions.detachLabel}
				onUpdateLabel={actions.updateLabel}
				onDeleteLabel={actions.deleteLabel}
			/>

			<DrawerToolbar
				hitCount={hits.count}
				hitIndex={hits.index}
				onprevhit={hits.prev}
				onnexthit={hits.next}
				bind:view
				autoApprove={session.auto_approve}
				ontoggleAuto={sa.toggleAutoApprove}
				pins={pins.pins}
				{lines}
				onjumpseq={(seq) => void pins.ensureSeqVisible(seq)}
				onunpin={pins.unpinSeq}
			/>

			<TaskPanel sessionId={id} progress={stream.todoProgress} />

			{#if terminalOpen}
				<TerminalPane sessionId={id} onclose={() => (terminalOpen = false)} />
			{/if}

			<DrawerBanners sessionId={id} {needsInput} {stream} bind:acctModalOpen />

			<Conversation
				bind:this={conv}
				{stream}
				{scroll}
				sessionId={id}
				{lines}
				isLoading={history.isLoading}
				canFetchOlder={earlier.canFetch}
				fetchingOlder={earlier.fetching}
				onfetcholder={earlier.fetchEarlier}
				{archived}
				askPreambleHtml={renderer.askPreambleHtml}
				planPreambleHtml={renderer.planPreambleHtml}
				onedit={editPending}
				onrespondperm={(rid, allow) => ws.respondPermission(id, rid, allow)}
				{forkable}
				selectMode={forkSelect.active}
				selected={forkSelect.selected}
				ontoggleselect={forkSelect.toggle}
				pinnedSeqs={pins.pinnedSeqs}
				onpin={pins.toggleLine}
				bind:jumper={pins.renderWindow}
				{focusTs}
				onbookmark={bookmarks.open}
				isBookmarked={bookmarks.isBookmarked}
				pluginActionsFor={(ln) => plugins.actionsFor(ln)}
				onpluginaction={(a) => plugins.openWith(a.pluginId, a.params)}
			/>

			<ActivityBanner {stream} {archived} />
			<AutoArchiveNotice {session} onpin={() => togglePin(session)} />

			<ConversationComposer
				bind:this={composer}
				{session}
				{archived}
				working={stream.working}
				{supportsAttachments}
				{scroll}
				onsend={(body) => stream.sendBody(body)}
				stageFiles={(files) => actions.stageFiles(id, files)}
				onNewFromScript={() => onNewFromScript?.(session)}
				onFork={fork.openDialog}
				onFollowup={onFollowup ? followup : undefined}
				onResume={sa.resume}
			/>
			</Dropzone>
		</div>

		{#if forkSelect.active}
			<ForkSelectBar
				count={forkSelect.selected.size}
				onfork={forkSelection}
				onforkall={fork.openDialog}
				oncancel={forkSelect.exit}
			/>
		{/if}

		{#if fork.open}
			<ForkModal
				{archived}
				{isCodexSession}
				parentTokens={fork.parentTokens}
				models={fork.models}
				efforts={fork.efforts}
				forking={fork.forking}
				extractLabel={fork.extractLabel}
				bind:model={fork.model}
				bind:effort={fork.effort}
				bind:prompt={fork.prompt}
				oncancel={fork.cancel}
				onsubmit={fork.submit}
			/>
		{/if}
	{/snippet}
</ResizablePanel>

{#if bookmarks.draft}
	<BookmarkSaveModal
		heading={m.bookmarks_save_title()}
		saveLabel={m.bookmarks_save_action()}
		title={bookmarks.draft.title}
		onsave={bookmarks.save}
		onclose={bookmarks.close}
	/>
{/if}
</div>

<style>
	/* Zero-size stacking-context host so the fixed panel and scrim paint on
	   the drawer layer instead of inside the page's own stacking order. */
	.drawer-host {
		position: relative;
		z-index: var(--z-drawer);
	}
	.drawer {
		display: flex;
		flex-direction: column;
		height: 100%;
		background: var(--bg);
		padding-top: var(--safe-top);
	}
	@media (max-width: 959px) {
		.drawer.plugin-open {
			display: none;
		}
	}
</style>
