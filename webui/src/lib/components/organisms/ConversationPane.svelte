<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { ws } from '#lib/ws.svelte.ts';
	import { Dropzone } from '@dorsk/tsumikit';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import DrawerHeader from './conversation/DrawerHeader.svelte';
	import DrawerToolbar from './conversation/DrawerToolbar.svelte';
	import ActivityBanner from './conversation/ActivityBanner.svelte';
	import DrawerBanners from './conversation/DrawerBanners.svelte';
	import OutcomeCard from './OutcomeCard.svelte';
	import AutoArchiveNotice from './conversation/AutoArchiveNotice.svelte';
	import TaskPanel from './conversation/TaskPanel.svelte';
	import SessionEventsPanel from './conversation/SessionEventsPanel.svelte';
	import TerminalPane from './conversation/TerminalPane.svelte';
	import Conversation from './conversation/Conversation.svelte';
	import ConversationComposer from './conversation/ConversationComposer.svelte';
	import ConversationSearchBar from './conversation/ConversationSearchBar.svelte';
	import PluginPaneHost from './conversation/PluginPaneHost.svelte';
	import LiveVoiceBar from './conversation/LiveVoiceBar.svelte';
	import PaneOverlays from './conversation/PaneOverlays.svelte';
	import { PaneModel } from './conversation/paneModel.svelte';
	import { guardEscape } from './conversation/escapeGuard';
	import { livenessClass } from './conversation/liveness';
	import { voiceTranscriptLanded } from './conversation/voiceMessage';
	import type { ConversationChrome } from './conversation/chrome';

	let {
		session: opened,
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
		/** Causal seq of the matched message when opened from a search hit. */
		focusSeq?: number | null;
		onNewFromScript?: (s: SessionListItem) => void;
		onFollowup?: (s: SessionListItem, instruction?: string) => void;
		/** Open another session in place, e.g. a fresh fork. */
		onNavigate?: (sessionId: string) => void;
		chrome?: ConversationChrome;
		/** Only the active pane answers the window keyboard chords. */
		active?: boolean;
		maximized?: boolean;
		onmaximize?: () => void;
	} = $props();

	const p = new PaneModel({
		get opened() {
			return opened;
		},
		get highlight() {
			return highlight;
		},
		get focusSeq() {
			return focusSeq;
		},
		get chrome() {
			return chrome;
		},
		get onclose() {
			return onclose;
		},
		get onFollowup() {
			return onFollowup;
		},
		get onNavigate() {
			return onNavigate;
		}
	});
	const { line, stream, search, sa, fork, forkSelect, pins, plugins, renderer, earlier, voice, actions } = p;
	const session = $derived(p.session);
	const id = $derived(p.id);
	const archived = $derived(p.archived);
	const lines = $derived(p.lines);
	const terminalOpen = $derived(p.terminalOpen);
	const tile = $derived(chrome === 'tile');
	const waitingSince = $derived(session.last_message_at ? Date.parse(session.last_message_at) : null);
	const showStatusBadge = $derived(session.status === 'new' || session.status === 'archived');

	const organising = $derived(hasFeature('organising'));
	const searchable = $derived(hasFeature('transcript_search'));
	const canFork = $derived(hasFeature('machine_spawn'));
	const forkable = $derived(canFork && p.harnessCaps.fork && !archived);
	const canFollowup = $derived(
		!!onFollowup && hasFeature('automation') && hasFeature('machine_spawn')
	);
	const canTerminal = $derived(p.harnessCaps.live_view && hasFeature('terminal'));
	const followup = (instruction?: string) => {
		if (canFollowup) onFollowup?.(session, instruction);
	};
</script>

<div class="conv-pane" data-chrome={chrome}>
	{#if plugins.current && plugins.open}
		{#key plugins.current.info.id}
			<PluginPaneHost
				plugin={plugins.current.info}
				module={plugins.current.module}
				{session}
				params={plugins.open.params}
				onclose={() => plugins.close()}
			/>
		{/key}
	{/if}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="drawer"
		class:plugin-open={plugins.current !== null}
		data-journey="conversation"
		onkeydown={guardEscape}
	>
		<Dropzone
			overlay
			multiple
			label={m.composer_drop_files()}
			disabled={!p.supportsAttachments || archived}
			onfiles={(f) => p.composer?.addFiles(f)}
			onactive={(a) => p.composer?.setDragActive(a)}
		>
			<DrawerHeader
				{session}
				{archived}
				working={stream.working}
				isCodexSession={p.isCodexSession}
				livenessClass={livenessClass(session)}
				{showStatusBadge}
				{maximized}
				compact={tile}
				{onmaximize}
				{onclose}
				onrename={sa.rename}
				onsetmodel={sa.setModel}
				oncopylink={sa.copyLink}
				oncopymarkdown={sa.copyMarkdown}
				onexport={sa.export}
				onsearch={searchable ? () => search.openBar() : undefined}
				onescape={search.escape}
				onescapeaction={tile && stream.working ? sa.interrupt : undefined}
				shortcuts={active}
				onfork={canFork ? fork.openDialog : undefined}
				onfollowup={canFollowup ? () => followup() : undefined}
				onforkselect={forkable ? forkSelect.toggleMode : undefined}
				forkSelectActive={forkSelect.active}
				oninterrupt={sa.interrupt}
				onarchive={sa.archive}
				onstoparchive={sa.stopAndArchive}
				onTogglePin={organising ? p.togglePin : undefined}
				onAccountClick={() => (p.acctModalOpen = true)}
				allLabels={p.allLabels}
				onCreateLabel={(name, color) => actions.createLabel(name, color, { session_id: id })}
				onAttachLabel={organising ? actions.attachLabel : undefined}
				onDetachLabel={actions.detachLabel}
				onUpdateLabel={actions.updateLabel}
				onDeleteLabel={actions.deleteLabel}
				onsetroom={hasFeature('rooms') ? p.setRoom : undefined}
				onclearroom={hasFeature('rooms') ? p.clearRoom : undefined}
				onthreadjump={p.thread.open}
			/>

			{#if !tile}
				<DrawerToolbar
					bind:view={p.view}
					autoApprove={session.auto_approve}
					ontoggleAuto={sa.toggleAutoApprove}
					{terminalOpen}
					ontoggleTerminal={canTerminal ? () => (p.terminalOpen = !terminalOpen) : undefined}
					plugins={plugins.buttons}
					pins={pins.pins}
					{lines}
					onjumpseq={organising ? (seq) => void pins.ensureSeqVisible(seq) : undefined}
					onunpin={organising ? pins.unpinSeq : undefined}
					sessionId={session.id}
					contextAttachable={!archived && p.owned}
				/>
			{/if}

			{#if search.open && searchable}
				<ConversationSearchBar {search} />
			{/if}

			<TaskPanel sessionId={id} progress={stream.todoProgress} />
			<SessionEventsPanel sessionId={id} />

			{#if terminalOpen && canTerminal}
				<TerminalPane sessionId={id} onclose={() => (p.terminalOpen = false)} />
			{/if}

			<DrawerBanners sessionId={id} needsInput={p.needsInput} {stream} bind:acctModalOpen={p.acctModalOpen} />
			<OutcomeCard outcome={session.latest_outcome} />

			<Conversation
				{stream}
				scroll={p.scroll}
				sessionId={id}
				viewKey={p.viewKey}
				{lines}
				isLoading={p.history.isLoading && lines.length === 0}
				canFetchOlder={earlier.canFetch || (p.threadShown && line.canFetchOlder)}
				fetchingOlder={earlier.fetching || line.fetching}
				onfetcholder={earlier.canFetch ? earlier.fetchEarlier : line.fetchOlder}
				anchor={p.thread.anchor}
				onopengeneration={(sid) => void p.thread.open(sid)}
				headHidden={renderer.headHidden}
				onrevealhead={p.revealHead}
				{archived}
				askPreambleHtml={renderer.askPreambleHtml}
				planPreambleHtml={renderer.planPreambleHtml}
				onedit={p.editPending}
				onrespondperm={(rid, allow, optionId) => ws.respondPermission(id, rid, allow, optionId)}
				{forkable}
				selectMode={forkSelect.active}
				selected={forkSelect.selected}
				ontoggleselect={forkSelect.toggle}
				pinnedSeqs={pins.pinnedSeqs}
				onpin={organising ? pins.toggleLine : undefined}
				bind:jumper={pins.renderWindow}
				focusTs={p.focusTs}
				onbookmark={organising ? p.bookmarks.open : undefined}
				isBookmarked={organising ? p.bookmarks.isBookmarked : undefined}
				pluginActionsFor={(ln) => plugins.actionsFor(ln)}
				onpluginaction={(a) => plugins.openWith(a.pluginId, a.params)}
				onquote={p.quoteLine}
				onspeak={voice.caps.tts ? voice.speak : undefined}
				speakingKey={voice.speakingKey}
			/>

			<ActivityBanner {stream} {archived} needsInput={p.needsInput} {waitingSince} />
			<AutoArchiveNotice {session} onpin={organising ? () => p.togglePin(session) : undefined} />

			{#if voice.live.active}
				<LiveVoiceBar
					voice={voice.live}
					onstop={voice.toggleLive}
					landed={voiceTranscriptLanded(lines, voice.live.transcript)}
				/>
			{/if}
			<ConversationComposer
				bind:this={p.composer}
				{session}
				{archived}
				lastAgentAt={stream.lastAgentAt}
				voiceEnabled={p.voiceEnabled}
				voiceLive={voice.live.active}
				onvoicelive={voice.caps.live ? voice.toggleLive : undefined}
				supportsAttachments={p.supportsAttachments}
				scroll={p.scroll}
				onsend={(body, dictated, followup) => stream.sendBody(body, dictated, followup)}
				stageFiles={(files) => actions.stageFiles(id, files)}
				onNewFromScript={() => onNewFromScript?.(session)}
				onFork={canFork ? fork.openDialog : undefined}
				onFollowup={canFollowup ? followup : undefined}
				onResume={canFork ? sa.resume : undefined}
				onfollowupstarted={p.thread.started}
				compact={tile}
			/>
		</Dropzone>
	</div>

	<PaneOverlays {p} {canFork} contained={tile} />
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
		overflow: clip;
		background: var(--bg);
		padding-top: var(--safe-top);
	}
	@media (max-width: 959px) {
		.drawer.plugin-open {
			display: none;
		}
	}
</style>
