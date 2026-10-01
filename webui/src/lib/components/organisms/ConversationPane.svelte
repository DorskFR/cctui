<script lang="ts">
	import { untrack } from 'svelte';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { AgentEvent } from '@bindings/AgentEvent';
	import { ws } from '$lib/ws.svelte';
	import { endpoints, useConversation, useSessionActions, useLabels, qk } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { clearSessionRoom, setSessionRoom, setSessionRoomByName } from '$lib/rooms';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { drafts, VIEW_OPTS } from '$lib/drafts';
	import { Dropzone } from '@dorsk/tsumikit';
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
	import type { Line, MsgCategory, ViewOpts } from './conversation/types';
	import { parseViewOpts } from './conversation/filters';
	import { lineMarkdown, mergeEventSources } from './conversation/format';
	import { ConversationStream, mergeLiveEvent } from './conversation/stream.svelte';
	import { ScrollController } from './conversation/scroll.svelte';
	import { ConversationSearch } from './conversation/convSearch.svelte';
	import ConversationSearchBar from './conversation/ConversationSearchBar.svelte';
	import { ForkController } from './conversation/fork.svelte';
	import { ForkSelection } from './conversation/forkSelect.svelte';
	import { SessionActions } from './conversation/sessionActions.svelte';
	import { EarlierPages } from './conversation/earlierPages.svelte';
	import { LineRenderer } from './conversation/lineRender.svelte';
	import { MessagePins } from './conversation/messagePins.svelte';
	import { BookmarkSaver } from './conversation/bookmarkSave.svelte';
	import { guardEscape } from './conversation/escapeGuard';
	import { livenessClass } from './conversation/liveness';
	import { notify } from '$lib/notify.svelte';
	import type { ConversationChrome } from './conversation/chrome';
	import { m } from '$lib/paraglide/messages';

	let {
		session,
		onclose,
		highlight = [],
		focusSeq = null,
		onNewFromScript,
		onFollowup,
		onNavigate,
		chrome = 'drawer',
		active = true,
		maximized = false,
		onmaximize
	}: {
		session: SessionListItem;
		/** Omitted in a tile, which has nothing to close back to. */
		onclose?: () => void;
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
		/** `drawer` keeps the back chevron and the document scroll lock; `tile`
		 *  swaps in close/maximize and leaves the page scrollable. */
		chrome?: ConversationChrome;
		/** False for the tiles that are not the active one: only the active pane
		 *  answers the window keyboard chords. */
		active?: boolean;
		maximized?: boolean;
		onmaximize?: () => void;
	} = $props();

	const id = $derived(session.id);
	const archived = $derived(session.status === 'archived');
	const needsInput = $derived(session.attention === 'needs_input' && !archived);
	const showStatusBadge = $derived(session.status === 'new' || session.status === 'archived');
	const qc = useQueryClient();

	// Read-only live terminal pane, toggled from the toolbar.
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

	// Room is a field on the session, so a change invalidates the session list
	// that the grouping and the card badge read.
	async function setRoom(sessionId: string, pick: { id: string } | { name: string }) {
		try {
			if ('id' in pick) await setSessionRoom(sessionId, pick.id);
			else await setSessionRoomByName(sessionId, pick.name);
			void qc.invalidateQueries({ queryKey: qk.sessionsAll });
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

	async function clearRoom(sessionId: string) {
		try {
			await clearSessionRoom(sessionId);
			void qc.invalidateQueries({ queryKey: qk.sessionsAll });
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}

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
	// Suppress this session's own toasts/sounds while it is on screen — ref-counted,
	// so a session open in both the drawer and a tile survives one of them closing.
	$effect(() => notify.holdOpen(id));

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
		// The find bar owns the highlight once it is open, so the /sessions-search
		// terms and a refined in-conversation query never fight over it.
		highlight: () => (search.open ? search.terms : highlight),
		events: () => events,
		scheduledTurns: () => scheduledTurnMap,
		pending: () => stream.pendingReplies,
		failed: () => stream.failedReplies,
		retrying: () => stream.retryingReplies,
		askPreamble: () => stream.ask?.preamble,
		planPreamble: () => stream.plan?.preamble
	});
	const lines = $derived(renderer.lines);
	function revealHead(categories: MsgCategory[]): void {
		const msgFilter = { ...view.msgFilter };
		for (const c of categories) msgFilter[c] = true;
		view = { ...view, msgFilter };
	}
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
	// Find-in-conversation. The hit list is the server's: the DOM only holds
	// the paged tail, so a `mark.search-hit` count can never be the total.
	const search = new ConversationSearch({
		id: () => id,
		fetchHits: (sid: string, q: string) => endpoints.conversationSearch(sid, q),
		ensureSeqVisible: (seq) => pins.ensureSeqVisible(seq),
		onerror: (e) => toasts.error(errMessage(e))
	});
	$effect(() => {
		const ev = stream.live.at(-1);
		if (ev) untrack(() => search.appendLive(ev));
	});
	$effect(() => {
		if (!stream.working) untrack(() => search.turnEnded());
	});
	// A session switch drops the older pages and hit cursor and closes a stale
	// terminal pane.
	$effect(() => {
		void id;
		earlier.reset();
		search.reset();
		terminalOpen = false;
		plugins.close();
	});

	// Opened from the /sessions search: the bar starts filled with those terms
	// so both flows run through one code path.
	$effect(() => {
		const seed = highlight.join(' ').trim();
		if (seed) untrack(() => search.openBar(seed));
	});

	const isCodexSession = $derived((session.adapter_id ?? '').startsWith('codex'));

	const sa = new SessionActions({
		id: () => id,
		session: () => session,
		events: () => events,
		view: () => view,
		actions,
		onclose: () => onclose?.()
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
			else onclose?.();
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
			send: (text) => composer?.sendText(text),
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

	function quoteLine(ln: Line, selection: string | null) {
		if (archived) return;
		void composer?.insertQuote(selection ?? lineMarkdown(ln));
	}

	const bookmarks = new BookmarkSaver({ id: () => id, sessionName: () => session.name ?? null });

	function followup(instruction?: string) {
		onFollowup?.(session, instruction);
	}

</script>

<div class="conv-pane" data-chrome={chrome}>
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
				{maximized}
				{onmaximize}
				{onclose}
				onrename={sa.rename}
				onsetmodel={sa.setModel}
				oncopylink={sa.copyLink}
				oncopymarkdown={sa.copyMarkdown}
				onexport={sa.export}
				onsearch={() => search.openBar()}
				onescape={search.escape}
				onescapeaction={chrome === 'tile' && stream.working ? sa.interrupt : undefined}
				shortcuts={active}
				onfork={fork.openDialog}
				onfollowup={onFollowup ? () => followup() : undefined}
				onforkselect={forkable ? forkSelect.toggleMode : undefined}
				forkSelectActive={forkSelect.active}
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
				onsetroom={setRoom}
				onclearroom={clearRoom}
			/>

			<DrawerToolbar
				bind:view
				autoApprove={session.auto_approve}
				ontoggleAuto={sa.toggleAutoApprove}
				{terminalOpen}
				ontoggleTerminal={() => (terminalOpen = !terminalOpen)}
				plugins={plugins.buttons}
				pins={pins.pins}
				{lines}
				onjumpseq={(seq) => void pins.ensureSeqVisible(seq)}
				onunpin={pins.unpinSeq}
			/>

			{#if search.open}
				<ConversationSearchBar {search} />
			{/if}

			<TaskPanel sessionId={id} progress={stream.todoProgress} />

			{#if terminalOpen}
				<TerminalPane sessionId={id} onclose={() => (terminalOpen = false)} />
			{/if}

			<DrawerBanners sessionId={id} {needsInput} {stream} bind:acctModalOpen />

			<Conversation
				{stream}
				{scroll}
				sessionId={id}
				{lines}
				isLoading={history.isLoading}
				canFetchOlder={earlier.canFetch}
				fetchingOlder={earlier.fetching}
				onfetcholder={earlier.fetchEarlier}
				headHidden={renderer.headHidden}
				onrevealhead={revealHead}
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
				onquote={quoteLine}
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
				contained={chrome === 'tile'}
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
	/* The positioning context every overlay inside the pane resolves against:
	   a tile is one of nine, so nothing may be centred on the viewport. */
	.conv-pane {
		position: relative;
		height: 100%;
		min-height: 0;
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
