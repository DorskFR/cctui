import { untrack } from 'svelte';
import { useQueryClient } from '@tanstack/svelte-query';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { AgentEvent } from '@bindings/AgentEvent';
import {
	endpoints,
	useConversation,
	useHarnesses,
	useLabels,
	useMachines,
	useMe,
	usePlugins,
	useSessionActions,
	qk
} from '#lib/queries/index.ts';
import { scheduledTurns, useScheduledMessages } from '#lib/queries/scheduled.ts';
import { toasts } from '#lib/toast.svelte.ts';
import { errMessage } from '#lib/api.ts';
import { clearSessionRoom, setSessionRoom, setSessionRoomByName } from '#lib/rooms.ts';
import { drafts, VIEW_OPTS } from '#lib/drafts.ts';
import { supportsAttachments as adapterSupportsAttachments } from '#lib/attachments.ts';
import { harnessCapabilities } from '#lib/harnesses.ts';
import { harnessTable } from '#lib/harnesses.svelte.ts';
import { registerComposer } from '#lib/plugins/composerBridge.svelte.ts';
import { DrawerPlugins } from '#lib/plugins/drawerPlugins.svelte.ts';
import { settings } from '#lib/settings.svelte.ts';
import { hasFeature } from '#lib/features.svelte.ts';
import { notify } from '#lib/notify.svelte.ts';
import type ConversationComposer from './ConversationComposer.svelte';
import type { ConversationChrome } from './chrome';
import type { Line, MsgCategory, ViewOpts } from './types';
import { parseViewOpts } from './filters';
import { lineMarkdown, mergeEventSources } from './format';
import { ConversationStream, mergeLiveEvent } from './stream.svelte';
import { ScrollController } from './scroll.svelte';
import { ConversationSearch } from './convSearch.svelte';
import { ForkController } from './fork.svelte';
import { ForkSelection } from './forkSelect.svelte';
import { SessionActions } from './sessionActions.svelte';
import { EarlierPages } from './earlierPages.svelte';
import { LineRenderer } from './lineRender.svelte';
import { FollowupLine } from './followupLine.svelte';
import { PENDING_MARK_KEY, generationEvents, instructionEvent, markerLine, ownLines, stripFirstTurn } from './threadView';
import { MessagePins } from './messagePins.svelte';
import { BookmarkSaver } from './bookmarkSave.svelte';
import { ownsSession } from './ownership';
import { PaneVoice } from './paneVoice.svelte';
import { ThreadLanding } from './threadLanding.svelte';

export interface PaneInputs {
	opened: SessionListItem;
	highlight: string[];
	focusSeq: number | null;
	chrome: ConversationChrome;
	onclose?: () => void;
	onFollowup?: (s: SessionListItem, instruction?: string) => void;
	onNavigate?: (sessionId: string) => void;
}

/** Construct during component init: it reads query context and owns effects. */
export class PaneModel {
	readonly #in: PaneInputs;
	readonly #qc = useQueryClient();
	readonly actions = useSessionActions();
	readonly line = new FollowupLine({
		thread: (sid, opts) => endpoints.threadConversation(sid, opts),
		tree: (sid) => endpoints.sessionThread(sid),
		session: (sid) => endpoints.session(sid)
	});
	readonly session: SessionListItem = $derived.by(() => {
		const opened = this.#in.opened;
		return this.line.headId && this.line.headId !== opened.id ? (this.line.head ?? opened) : opened;
	});
	// Moves only when the opened session leaves the line, so a head swap keeps
	// scroll, render window and panes.
	viewKey = $state('');
	readonly id = $derived(this.session.id);
	readonly archived = $derived(this.session.status === 'archived');

	readonly #me = useMe();
	readonly #machines = useMachines(
		() => this.#me.data?.user_id ?? '',
		() => this.#me.data?.role === 'admin' && !!this.#me.data?.user_id && hasFeature('machine_spawn')
	);
	readonly owned = $derived(ownsSession(this.#me.data, this.#machines.data, this.session.machine_id));
	readonly needsInput = $derived(this.session.attention === 'needs_input' && !this.archived);

	readonly #installedPlugins = usePlugins();
	readonly plugins = new DrawerPlugins({
		sessionId: () => this.id,
		installed: () => (hasFeature('plugins') ? (this.#installedPlugins.data ?? []) : []),
		enabled: () => settings.pluginsEnabled
	});
	terminalOpen = $state(false);
	acctModalOpen = $state(false);
	view = $state<ViewOpts>(parseViewOpts(drafts.get(VIEW_OPTS)));

	readonly history = useConversation(() => this.id, () => true);
	readonly #labels = useLabels();
	readonly allLabels = $derived(this.#labels.data?.labels ?? []);

	readonly scroll = new ScrollController();
	readonly stream = new ConversationStream({
		id: () => this.id,
		archived: () => this.archived,
		historyData: () => this.history.data,
		pin: this.scroll.stickToBottom,
		invalidateConversation: () => this.#qc.invalidateQueries({ queryKey: qk.conversation(this.id) }),
		invalidateSessions: () => this.#qc.invalidateQueries({ queryKey: qk.sessionsAll }),
		mergeIntoCache: (sid, ev) =>
			this.#qc.setQueryData<AgentEvent[]>(qk.conversation(sid), (prev) => mergeLiveEvent(prev, ev)),
		sessionTodos: () => this.session.todos
	});

	readonly earlier: EarlierPages = new EarlierPages({
		id: () => this.id,
		historyLength: () => this.history.data?.length ?? 0,
		events: () => this.events
	});
	readonly events: AgentEvent[] = $derived(
		mergeEventSources(this.history.data ?? [], this.earlier.pages, this.stream.live)
	);
	readonly headComplete = $derived(
		!this.earlier.canFetch && (this.history.data !== undefined || this.line.fresh)
	);
	readonly threadShown = $derived(this.line.active && this.headComplete);
	readonly #renderEvents = $derived.by(() => {
		if (this.line.headGen < 2) return this.events;
		return this.headComplete ? generationEvents(this.events, this.line.headMark) : stripFirstTurn(this.events, null);
	});
	// `Line` carries no seq, so the focused line is addressed by `ts`.
	readonly focusTs = $derived.by(() => {
		const seq = this.#in.focusSeq;
		if (seq == null) return null;
		const ev = this.events.find((e) => e.seq === seq);
		return ev ? Number(ev.ts) : null;
	});

	readonly #scheduled = useScheduledMessages(() => this.id);
	readonly #scheduledTurns = $derived(scheduledTurns(this.#scheduled.data));
	readonly renderer = new LineRenderer({
		id: () => this.id,
		machineId: () => (hasFeature('files') ? this.session.machine_id : ''),
		view: () => this.view,
		// The open find bar owns the highlight, so it never fights the search terms.
		highlight: () => (this.search.open ? this.search.terms : this.#in.highlight),
		events: () => this.#renderEvents,
		prior: () => (this.threadShown ? this.line.segments : []),
		scheduledTurns: () => this.#scheduledTurns,
		pending: () => this.stream.pendingReplies,
		failed: () => this.stream.failedReplies,
		retrying: () => this.stream.retryingReplies,
		askPreamble: () => this.stream.ask?.preamble,
		planPreamble: () => this.stream.plan?.preamble
	});
	readonly headLines = $derived(ownLines(this.renderer.lines, this.id, this.line.headGen));
	readonly #pendingLines = $derived.by(() => {
		const mark = this.line.pending;
		if (!mark) return [];
		const ev = instructionEvent(mark);
		const own = ev ? this.renderer.segmentLines({ sessionId: PENDING_MARK_KEY, events: [ev] }) : [];
		return [markerLine(mark), ...own.map((l) => ({ ...l, key: `${PENDING_MARK_KEY}:${l.key}` }))];
	});
	readonly lines: Line[] = $derived([
		...this.renderer.priorLines,
		...(this.threadShown && this.line.headGen >= 2 && this.line.headMark ? [markerLine(this.line.headMark)] : []),
		...this.headLines,
		...this.#pendingLines
	]);

	readonly pins = new MessagePins({
		id: () => this.id,
		events: () => this.events,
		canFetchOlder: () => this.earlier.canFetch,
		fetchOlder: this.earlier.fetchEarlier,
		scroll: this.scroll
	});
	// The DOM only holds the paged tail, so the hit list is the server's.
	readonly search = new ConversationSearch({
		id: () => this.id,
		fetchHits: (sid: string, q: string, signal?: AbortSignal) =>
			endpoints.conversationSearch(sid, q, undefined, signal),
		ensureSeqVisible: (seq) => this.pins.ensureSeqVisible(seq),
		onerror: (e) => toasts.error(errMessage(e))
	});

	readonly isCodexSession = $derived((this.session.adapter_id ?? '').startsWith('codex'));
	readonly harnessCaps = $derived(harnessCapabilities(this.session.adapter_id, harnessTable()));
	readonly sa = new SessionActions({
		id: () => this.id,
		session: () => this.session,
		events: () => this.events,
		view: () => this.view,
		actions: this.actions,
		onclose: () => this.#in.onclose?.()
	});
	readonly fork = new ForkController({
		id: () => this.id,
		archived: () => this.archived,
		isCodex: () => this.isCodexSession,
		session: () => this.session,
		fork: (sid, body) => this.actions.fork(sid, body),
		// Jump to the new conversation when claude returned its id; otherwise
		// close and let the list refetch surface it.
		onForked: (sid) => {
			const { onNavigate, onclose } = this.#in;
			if (sid && onNavigate) onNavigate(sid);
			else onclose?.();
		}
	});
	readonly forkSelect = new ForkSelection();
	readonly supportsAttachments = $derived(
		hasFeature('uploads') && adapterSupportsAttachments(this.session.adapter_id)
	);
	composer = $state<ConversationComposer>();
	readonly bookmarks = new BookmarkSaver({ id: () => this.id, sessionName: () => this.session.name ?? null });
	readonly voice: PaneVoice;
	readonly voiceEnabled = $derived.by(() => this.voice.caps.stt && !this.archived);
	readonly thread: ThreadLanding;

	/** `inputs` holds one getter per prop, so each effect tracks only the props it reads. */
	constructor(inputs: PaneInputs) {
		this.#in = inputs;
		this.viewKey = untrack(() => inputs.opened.id);
		useHarnesses();
		this.#effects();
		this.voice = new PaneVoice(() => this.id);
		this.thread = new ThreadLanding({
			line: this.line,
			earlier: this.earlier,
			id: () => this.id,
			session: () => this.session,
			events: () => this.events,
			lines: () => this.lines,
			headComplete: () => this.headComplete,
			historyLoaded: () => this.history.data !== undefined,
			viewKey: () => this.viewKey,
			keepUrl: () => this.#in.chrome === 'tile',
			jumpToBottom: () => this.scroll.jumpToBottom(),
			restoreDraft: (text) => this.composer?.restoreDraft(text)
		});
	}

	#effects() {
		$effect(() => {
			const sid = this.#in.opened.id;
			if (!untrack(() => this.line.members.has(sid))) this.viewKey = sid;
		});
		$effect(() => {
			void this.viewKey;
			untrack(() => void this.line.open(this.#in.opened));
		});
		$effect(() => this.plugins.ensureLoaded());
		$effect(() => {
			drafts.set(VIEW_OPTS, JSON.stringify(this.view));
		});
		$effect(() => {
			const sid = this.id;
			void this.stream.resubTick;
			return this.stream.subscribe(sid);
		});
		$effect(() => this.stream.installVisibilityRefresh());
		$effect(() => notify.holdOpen(this.id));
		$effect(() => {
			const seq = this.#in.focusSeq;
			const sid = this.id;
			if (seq != null) void this.earlier.focus(sid, seq);
		});
		$effect(() => {
			void this.lines.length;
			void this.plugins.ready.length;
			this.plugins.autoOpenFrom(this.lines);
		});
		$effect(() => {
			void this.lines.length;
			void this.stream.perms.length;
			void this.stream.working;
			this.scroll.followIfStuck();
		});
		// A search hit must land mid-transcript and stay there.
		$effect(() => {
			void this.viewKey;
			if (this.#in.focusSeq == null) untrack(() => this.scroll.resetForSession());
			else untrack(() => this.scroll.unstick());
		});
		$effect(() => this.scroll.observeResize());
		$effect(() => {
			const ev = this.stream.live.at(-1);
			if (ev) untrack(() => this.search.appendLive(ev));
		});
		$effect(() => {
			if (!this.stream.working) untrack(() => this.search.turnEnded());
		});
		// Older pages and search hits belong to one session; the terminal and
		// plugin panes survive a head swap.
		$effect(() => {
			void this.id;
			this.earlier.reset();
			this.search.reset();
		});
		$effect(() => {
			void this.viewKey;
			this.terminalOpen = false;
			untrack(() => this.plugins.close());
		});
		$effect(() => {
			const seed = hasFeature('transcript_search') ? this.#in.highlight.join(' ').trim() : '';
			if (seed) untrack(() => this.search.openBar(seed));
		});
		$effect(() =>
			registerComposer(this.id, {
				insertText: (text) => void this.composer?.insertText(text),
				send: (text) => this.composer?.sendText(text),
				addFiles: (files) => this.composer?.addFiles(files),
				focus: () => this.composer?.focus()
			})
		);
	}

	togglePin = (s: SessionListItem) => (s.pinned ? this.actions.unpin(s.id) : this.actions.pin(s.id));

	setRoom = async (sessionId: string, pick: { id: string } | { name: string }) => {
		try {
			if ('id' in pick) await setSessionRoom(sessionId, pick.id);
			else await setSessionRoomByName(sessionId, pick.name);
			void this.#qc.invalidateQueries({ queryKey: qk.sessionsAll });
		} catch (e) {
			toasts.error(errMessage(e));
		}
	};

	clearRoom = async (sessionId: string) => {
		try {
			await clearSessionRoom(sessionId);
			void this.#qc.invalidateQueries({ queryKey: qk.sessionsAll });
		} catch (e) {
			toasts.error(errMessage(e));
		}
	};

	revealHead = (categories: MsgCategory[]) => {
		const msgFilter = { ...this.view.msgFilter };
		for (const c of categories) msgFilter[c] = true;
		this.view = { ...this.view, msgFilter };
	};

	forkSelection = () => {
		const range = this.forkSelect.range(this.headLines);
		if (range.length === 0) return;
		this.fork.openExtract({ mode: 'selected', anchor_message_id: null, selected_message_ids: range });
	};

	editPending = (text: string, ts: number) => {
		if (this.archived) return;
		const typed = this.stream.discardOptimistic(ts);
		this.composer?.loadDraft(typed ?? text);
	};

	quoteLine = (ln: Line, selection: string | null) => {
		if (this.archived) return;
		void this.composer?.insertQuote(selection ?? lineMarkdown(ln));
	};
}
