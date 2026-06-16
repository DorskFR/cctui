<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { AgentEvent } from '@bindings/AgentEvent';
	import { ws, USER_PREFIX } from '$lib/ws.svelte';
	import { useConversation, useSessionActions, useLabels, qk } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { renderMarkdown, highlightBlock } from '$lib/markdown';
	import { highlightTerms } from '$lib/search';
	import { drafts, VIEW_OPTS } from '$lib/drafts';
	import { Dropzone } from '@dorsk/tsumikit';
	import BackdropScrim from './conversation/BackdropScrim.svelte';
	import ForkModal from './conversation/ForkModal.svelte';
	import DrawerHeader from './conversation/DrawerHeader.svelte';
	import DrawerToolbar from './conversation/DrawerToolbar.svelte';
	import Conversation from './conversation/Conversation.svelte';
	import ConversationComposer from './conversation/ConversationComposer.svelte';
	import { MSG_TYPES, type MsgType, type ViewOpts, type Line } from './conversation/types';
	import { looksMeta, parseAsk, parsePlan, eventSig, formatToolInput, orderAskTurns } from './conversation/format';
	import { ConversationStream } from './conversation/stream.svelte';
	import { ScrollController } from './conversation/scroll.svelte';
	import { ForkController } from './conversation/fork.svelte';
	import { SessionActions } from './conversation/sessionActions.svelte';

	let {
		session,
		onclose,
		highlight = [],
		onNewFromScript,
		onNavigate
	}: {
		session: SessionListItem;
		onclose: () => void;
		highlight?: string[];
		// "New session from same script" for archived sessions (CCT-250 item 8).
		onNewFromScript?: (s: SessionListItem) => void;
		// Open another session in place by id (CCT-345) — used to jump straight to a
		// freshly forked conversation without a manual refresh.
		onNavigate?: (sessionId: string) => void;
	} = $props();

	// Search terms to highlight inline (CCT-187), set when opened from a search.
	const hl = (html: string) => (highlight.length ? highlightTerms(html, highlight) : html);

	const id = $derived(session.id);
	const archived = $derived(session.status === 'archived');
	const needsInput = $derived(session.attention === 'needs_input' && !archived);
	// Liveness dot next to the title (CCT-311), mirroring SessionCard.
	const livenessClass = $derived(
		session.hibernated
			? 'dot-hibernated'
			: session.liveness === 'active'
				? 'dot-active'
				: session.liveness === 'stale'
					? 'dot-stale'
					: 'dot-dead'
	);
	const showStatusBadge = $derived(session.status === 'new' || session.status === 'archived');
	const qc = useQueryClient();

	// Message-type tag filter (CCT-250 item 2), shared types in ./conversation/types.
	const defaults: ViewOpts = {
		typeFilter: {
			assistant: 'off',
			user: 'off',
			tool: 'off',
			mcp: 'exclude',
			system: 'off',
			result: 'off'
		},
		prettyJson: true,
		prettyDiff: true,
		prettyTables: true,
		paneWidth: null
	};
	const PANE_MIN = 360; // px — narrowest the drawer can be dragged
	let view = $state<ViewOpts>(loadView());
	function loadView(): ViewOpts {
		try {
			const saved = JSON.parse(drafts.get(VIEW_OPTS) || '{}');
			return {
				...defaults,
				...saved,
				// typeFilter is nested — merge per-key so a partial/old payload keeps
				// the sensible defaults.
				typeFilter: { ...defaults.typeFilter, ...(saved.typeFilter ?? {}) }
			};
		} catch {
			return { ...defaults };
		}
	}
	// Whether a given message type passes the current tag filter.
	const anyIncluded = $derived(MSG_TYPES.some((m) => view.typeFilter[m.id] === 'include'));
	function typeVisible(t: MsgType): boolean {
		const st = view.typeFilter[t];
		if (st === 'exclude') return false;
		if (anyIncluded) return st === 'include';
		return true;
	}
	$effect(() => {
		drafts.set(VIEW_OPTS, JSON.stringify(view));
	});

	const history = useConversation(
		() => id,
		() => true
	);
	const actions = useSessionActions();

	// Labels (CCT-360) + pin (CCT-267) in the drawer header — the same global
	// label set and mutations the session list uses, so editing a session's
	// labels/star from the open conversation stays in sync with the list.
	const labelsQuery = useLabels();
	const allLabels = $derived($labelsQuery.data?.labels ?? []);
	const createLabel = (name: string, color: string) => actions.createLabel(name, color);
	const attachLabel = (sid: string, labelId: string) => actions.attachLabel(sid, labelId);
	const detachLabel = (sid: string, labelId: string) => actions.detachLabel(sid, labelId);
	const updateLabel = (labelId: string, patch: { name?: string; color?: string }) =>
		actions.updateLabel(labelId, patch);
	const deleteLabel = (labelId: string) => actions.deleteLabel(labelId);
	const togglePin = (s: SessionListItem) => (s.pinned ? actions.unpin(s.id) : actions.pin(s.id));

	// ── Sticky-bottom scroll controller (CCT-161) ──────────────────────────
	// Shared by the viewport (binds the scroller) and the composer (binds the
	// textarea, whose growth must re-pin the viewport).
	const scroll = new ScrollController();

	// ── WS subscription + live events + send orchestration ──────────────────
	// Owns the live buffer, optimistic echoes, permission/ask/delivery state and
	// the "Working…" activity indicator (see conversation/stream.svelte.ts).
	const stream = new ConversationStream({
		id: () => id,
		archived: () => archived,
		historyData: () => $history.data,
		pin: scroll.stickToBottom,
		invalidateConversation: () => qc.invalidateQueries({ queryKey: qk.conversation(id) }),
		invalidateSessions: () => qc.invalidateQueries({ queryKey: ['sessions'] })
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

	// History (fetched) + live (ws) events, merged in order, with live events
	// already present in history dropped so a reconnect/focus refetch and the
	// persisted form of an optimistic reply don't render twice. The WHOLE merged
	// list is ordered by `ts` with a stable sort so an optimistic reply that
	// survives a refetch lands in its correct chronological place (CCT-186);
	// equal-`ts` ties keep their original order (history stays ahead of a live
	// event sharing its `ts`).
	const events = $derived.by(() => {
		const hist = $history.data ?? [];
		const seen = new Set(hist.map(eventSig));
		const tail = stream.live.filter((e) => !seen.has(eventSig(e)));
		return [...hist, ...tail].sort((a, b) => a.ts - b.ts);
	});

	// ── Line building (parse + filter + dedup + delivery tinting) ───────────
	// Render markdown honoring the table formatting toggle (CCT-250 item 2).
	const mdRender = (s: string) => hl(renderMarkdown(s, { tables: view.prettyTables }));
	// History stores user turns as a `text` event prefixed with USER_PREFIX; some
	// "user" turns are really harness/system messages (marked via `meta`, with a
	// tag fallback for pre-`meta` rows) and render in a distinct hue.
	function userOrSystem(content: string, ts: number, meta: boolean): Line | null {
		const role = meta ? 'system' : 'user';
		if (!typeVisible(role)) return null;
		return { role, ts, html: mdRender(content), text: content };
	}

	function toLine(e: AgentEvent): Line | null {
		switch (e.type) {
			case 'text': {
				// Streaming emits an empty text event before the populated one — skip
				// empties so they don't render as blank assistant blocks.
				if (!e.content.trim()) return null;
				if (e.content.startsWith(USER_PREFIX)) {
					const content = e.content.slice(USER_PREFIX.length).trimStart();
					return userOrSystem(content, Number(e.ts), e.meta || looksMeta(content));
				}
				if (!typeVisible('assistant')) return null;
				return {
					role: 'assistant',
					ts: Number(e.ts),
					html: mdRender(e.content),
					text: e.content,
					messageId: e.message_id,
					usage: e.usage
				};
			}
			case 'reply':
				// `reply` is only ever our own optimistic echo of typed input.
				if (!e.content.trim()) return null;
				return userOrSystem(e.content, Number(e.ts), false);
			case 'tool_call': {
				// AskUserQuestion (CCT-146): render as interactive cards, not raw JSON.
				if (e.tool === 'AskUserQuestion') {
					const ask = parseAsk(e.input);
					if (ask) return { role: 'tool', ts: Number(e.ts), tool: e.tool, ask };
				}
				// ExitPlanMode (CCT-347): render the plan + continuations as a Plan card.
				if (e.tool === 'ExitPlanMode') {
					const plan = parsePlan(e.input);
					if (plan) return { role: 'tool', ts: Number(e.ts), tool: e.tool, plan };
				}
				const isMcp = e.tool.startsWith('mcp__');
				// MCP tool calls filter on the 'mcp' tag; other tool calls on 'tool'.
				if (!typeVisible(isMcp ? 'mcp' : 'tool')) return null;
				const { text, lang } = formatToolInput(e.tool, e.input, {
					prettyDiff: view.prettyDiff,
					prettyJson: view.prettyJson
				});
				return {
					role: 'tool',
					ts: Number(e.ts),
					tool: e.tool,
					mcp: isMcp,
					text,
					lang,
					htmlCode: hl(highlightBlock(text, lang))
				};
			}
			case 'tool_result':
				if (!typeVisible('result')) return null;
				return {
					role: 'result',
					ts: Number(e.ts),
					tool: e.tool,
					text: e.output_summary,
					htmlCode: hl(highlightBlock(e.output_summary, ''))
				};
			case 'context_reset':
				// /clear: the session id rotated under the same worker (CCT-158).
				return { role: 'reset', ts: Number(e.ts), text: 'context reset · /clear or /compact' };
			case 'compact_summary':
				// /compact appends a summary in place (no session-id rotation), so it
				// arrives with its text (CCT-159).
				if (!e.content.trim()) return null;
				return { role: 'compact', ts: Number(e.ts), html: mdRender(e.content), text: e.content };
			default:
				return null; // heartbeat, turn_end
		}
	}

	// Build lines with consecutive-duplicate dedup, tinting user lines with their
	// per-message delivery state (pending/retrying/failed, CCT-212 → CCT-214).
	const lines = $derived.by(() => {
		const pendingTs = stream.pendingReplies;
		const failedTs = stream.failedReplies;
		const retryingTs = stream.retryingReplies;
		const out: Line[] = [];
		let prevKey = '';
		for (const e of events) {
			const ln = toLine(e);
			if (!ln) continue;
			// Reset/compact markers are keyed by ts so two back-to-back ones aren't
			// collapsed by the consecutive-duplicate guard.
			const key =
				ln.role === 'reset' || ln.role === 'compact'
					? `${ln.role}|${ln.ts}`
					: `${ln.role}|${ln.tool ?? ''}|${ln.text ?? ln.html ?? ''}`;
			if (key === prevKey) continue;
			prevKey = key;
			if (ln.role === 'user') {
				if (pendingTs.has(ln.ts)) ln.pending = true;
				const retry = retryingTs.get(ln.ts);
				if (retry !== undefined) ln.retrying = retry;
				const reason = failedTs.get(ln.ts);
				if (reason !== undefined) ln.failed = reason;
			}
			out.push(ln);
		}
		// Re-anchor any AskUserQuestion turn to its causal order before computing
		// per-line durations, so a late-arriving preamble + ask card (stamped with
		// receive-time ts after the user's answer) render above the answer rather
		// than below it (CCT-338).
		const ordered = orderAskTurns(out);
		for (let i = 0; i < ordered.length; i++) {
			if (ordered[i].role !== 'assistant') continue;
			const prev = [...ordered.slice(0, i)]
				.reverse()
				.find((l) => l.role === 'user' || l.role === 'assistant');
			if (prev && ordered[i].ts > prev.ts) ordered[i].durationMs = ordered[i].ts - prev.ts;
		}
		return ordered;
	});
	// The assistant prose preceding the live question (CCT-213), rendered as
	// markdown above the card so the user answers with context, not blind.
	const askPreambleHtml = $derived(
		stream.ask?.preamble ? hl(renderMarkdown(stream.ask.preamble)) : null
	);
	// The assistant prose preceding the live plan (CCT-347), same treatment.
	const planPreambleHtml = $derived(
		stream.plan?.preamble ? hl(renderMarkdown(stream.plan.preamble)) : null
	);

	// ── Scroll wiring (content-follow, session reset, composer-growth observer) ─
	// Only follow new content when the user is pinned to the bottom.
	$effect(() => {
		void lines.length;
		void stream.perms.length;
		void stream.working;
		scroll.followIfStuck();
	});
	// Reset to bottom + sticky when switching sessions.
	$effect(() => {
		void id;
		scroll.resetForSession();
	});
	// Keep pinned to the bottom while the composer grows (CCT-161). Re-runs when
	// the scroller / textarea attach (the controller reads both reactively).
	$effect(() => scroll.observeResize());

	const isCodexSession = $derived((session.adapter_id ?? '').startsWith('codex'));

	// ── Session actions (rename / archive / interrupt / resume / model switch /
	// auto-approve / export / copy) ─────────────────────────────────────────
	// Thin wrappers over the query layer + export helpers, collected into one
	// controller so the drawer stays a composition shell and the header/composer
	// stay presentational (conversation/sessionActions.svelte.ts).
	const sa = new SessionActions({
		id: () => id,
		session: () => session,
		events: () => events,
		view: () => view,
		actions,
		onclose: () => onclose()
	});

	// ── Fork conversation (CCT-302) ───────────────────────────────────────────
	// Self-contained hook (conversation/fork.svelte.ts): also the supported
	// "switch model" substitute for claude (CCT-303) and the "reopen" path for
	// archived sessions.
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

	// Mid-chat file attachments are supported on filesystem-backed adapters
	// (CCT-236); the composer owns the attachment state, the viewport's dropzone
	// feeds it via the component ref.
	const supportsAttachments = $derived(
		session.adapter_id === 'claude-code' || session.adapter_id === 'codex'
	);
	let composer = $state<ConversationComposer>();

	// Edit a still-pending message (CCT-208): drop the in-flight echo and pull its
	// text back into the composer to fix and resend.
	function editPending(text: string, ts: number) {
		if (archived) return;
		stream.discardOptimistic(ts);
		composer?.loadDraft(text);
	}

	// Mobile chat controls collapse behind text buttons that open popovers
	// (CCT-311); null = no panel open. Desktop shows the controls inline.
	let mobilePanel = $state<'filters' | 'format' | 'auto' | null>(null);
	// The agent-side worker is gone once archived, so re-dispatch a fresh session
	// seeded with this one's config rather than trying to revive it.
	function newFromScript() {
		onNewFromScript?.(session);
	}

	// ── Drag-to-resize the desktop drawer (left border) ─────────────────────
	let resizing = $state(false);
	// Coalesce pointermoves to one width update per frame (mirrors tsumikit's
	// Modal): pointer events fire faster than the refresh and each width change
	// reflows + repaints the pane, so writing once per rAF caps that to the frame
	// rate and keeps the drag smooth instead of jaggery.
	let rafId = 0;
	let lastX = 0;
	function startResize(e: PointerEvent) {
		resizing = true;
		lastX = e.clientX;
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		e.preventDefault();
	}
	function onResize(e: PointerEvent) {
		if (!resizing) return;
		lastX = e.clientX;
		if (rafId) return;
		rafId = requestAnimationFrame(() => {
			rafId = 0;
			const w = window.innerWidth - lastX;
			view.paneWidth = Math.round(Math.max(PANE_MIN, Math.min(w, window.innerWidth)));
		});
	}
	function endResize(e: PointerEvent) {
		if (!resizing) return;
		resizing = false;
		if (rafId) {
			cancelAnimationFrame(rafId);
			rafId = 0;
		}
		try {
			(e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
		} catch {
			/* pointer already released */
		}
	}
	const drawerWidth = $derived(
		view.paneWidth ? `min(${view.paneWidth}px, 100vw)` : 'min(900px, 100vw)'
	);
</script>

<BackdropScrim {onclose} />

<div class="drawer" class:resizing style="--drawer-width: {drawerWidth}">
	<!-- Drag the left border to resize the desktop side-pane (CCT-161). -->
	<div
		class="resize-handle"
		role="separator"
		aria-label="Resize panel"
		aria-orientation="vertical"
		onpointerdown={startResize}
		onpointermove={onResize}
		onpointerup={endResize}
		onpointercancel={endResize}
	></div>
	<!-- The whole drawer is a file drop area (CCT-236): dragging files over it
	     shows the tsumikit Dropzone overlay; on drop they're staged as composer
	     attachments. overlay mode wraps the content without hijacking clicks. -->
	<Dropzone
		overlay
		multiple
		label="Drop files to attach"
		disabled={!supportsAttachments || archived}
		onfiles={(f) => composer?.addFiles(f)}
		onactive={(a) => composer?.setDragActive(a)}
	>
		<DrawerHeader
		{session}
		{archived}
		{isCodexSession}
		{livenessClass}
		{showStatusBadge}
		{onclose}
		onrename={sa.rename}
		onsetmodel={sa.setModel}
		oncopylink={sa.copyLink}
		oncopymarkdown={sa.copyMarkdown}
		onexport={sa.export}
		onfork={fork.openDialog}
		oninterrupt={sa.interrupt}
		onarchive={sa.archive}
		onTogglePin={togglePin}
		{allLabels}
		onCreateLabel={createLabel}
		onAttachLabel={attachLabel}
		onDetachLabel={detachLabel}
		onUpdateLabel={updateLabel}
		onDeleteLabel={deleteLabel}
	/>

	<DrawerToolbar
		bind:view
		autoApprove={session.auto_approve}
		bind:mobilePanel
		ontoggleAuto={sa.toggleAutoApprove}
	/>

	{#if needsInput}
		<div class="attn-banner">✋ Waiting for your input</div>
	{/if}

	<Conversation
		{scroll}
		sessionId={id}
		{lines}
		isLoading={$history.isLoading}
		{archived}
		perms={stream.perms}
		ask={stream.ask}
		liveAskQuestions={stream.liveAskQuestions}
		{askPreambleHtml}
		working={stream.working}
		answering={stream.answering}
		isDupeOfLiveAsk={stream.isDupeOfLiveAsk}
		plan={stream.plan}
		{planPreambleHtml}
		onanswer={(t, p, qs) => stream.answerQuestion(t, p, qs)}
		onanswerplan={(t, p) => stream.answerPlan(t, p)}
		onretry={(ts) => stream.retryFailed(ts)}
		onedit={editPending}
		onrespondperm={(rid, allow) => ws.respondPermission(id, rid, allow)}
	/>

	<ConversationComposer
		bind:this={composer}
		{session}
		{archived}
		working={stream.working}
		{supportsAttachments}
		{scroll}
		onsend={(body) => stream.sendBody(body)}
		stageFiles={(files) => actions.stageFiles(id, files)}
		onNewFromScript={newFromScript}
		onFork={fork.openDialog}
		onResume={sa.resume}
	/>
	</Dropzone>
</div>

{#if fork.open}
	<ForkModal
		{archived}
		{isCodexSession}
		parentTokens={fork.parentTokens}
		models={fork.models}
		efforts={fork.efforts}
		forking={fork.forking}
		bind:model={fork.model}
		bind:effort={fork.effort}
		oncancel={fork.cancel}
		onsubmit={fork.submit}
	/>
{/if}

<style>
	.drawer {
		position: fixed;
		inset: 0;
		z-index: var(--z-drawer);
		background: var(--bg);
		display: flex;
		flex-direction: column;
		padding-top: var(--safe-top);
		animation: slide 0.18s var(--ease);
	}
	/* Full-width on narrow viewports; a right-anchored side pane on wide ones. */
	@media (min-width: 960px) {
		.drawer {
			left: auto;
			right: 0;
			width: var(--drawer-width, min(900px, 100vw));
			border-left: 1px solid var(--border);
			box-shadow: -4px 0 24px rgba(0, 0, 0, 0.4);
		}
	}
	/* While dragging the resize handle, suppress text selection / the slide-in
	   animation so the pane tracks the pointer cleanly. */
	.drawer.resizing {
		user-select: none;
		animation: none;
		/* Make per-frame width changes cheap to paint while dragging (mirrors the
		   Modal): hint the animated property and isolate layout/paint to the pane. */
		will-change: width;
		contain: layout paint;
	}
	/* Drag handle on the left border — desktop only (mobile is full-width). */
	.resize-handle {
		display: none;
	}
	@media (min-width: 960px) {
		.resize-handle {
			display: block;
			position: absolute;
			top: 0;
			bottom: 0;
			left: 0;
			width: 10px;
			margin-left: -5px;
			z-index: 4;
			cursor: col-resize;
			touch-action: none;
		}
		/* Persistent grip hint (mirrors tsumikit's Modal): a small pill centered on
		   the handle, brightening to the accent on hover / while dragging. */
		.resize-handle::after {
			content: '';
			position: absolute;
			top: 50%;
			left: 50%;
			transform: translate(-50%, -50%);
			width: 3px;
			height: 28px;
			border-radius: 999px;
			background: var(--border-strong);
			transition: background 0.12s var(--ease);
		}
		.resize-handle:hover::after,
		.drawer.resizing .resize-handle::after {
			background: var(--accent);
		}
	}
	@keyframes slide {
		from {
			transform: translateX(4%);
			opacity: 0.5;
		}
	}
	.attn-banner {
		padding: var(--sp-2) var(--sp-3);
		background: var(--attention-bg);
		border-bottom: 1px solid var(--attention-bar);
		color: var(--warn);
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
	}
</style>
