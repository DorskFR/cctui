<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { AgentEvent } from '@bindings/AgentEvent';
	import { ws, userMsgKey, USER_PREFIX, type PermReq, type LiveAsk } from '$lib/ws.svelte';
	import { useConversation, useSessionActions } from '$lib/queries';
	import { renderMarkdown, prettyJson, highlightBlock } from '$lib/markdown';
	import { highlightTerms } from '$lib/search';
	import { clockTime, statusBadgeClass, compact } from '$lib/format';
	import { drafts, composerKey, history as msgHistory, clearSessionStorage, VIEW_OPTS } from '$lib/drafts';
	import { autoresize } from '$lib/autoresize';
	import { dropzone } from '$lib/dropzone';
	import { mergeFiles, removeFileByName, fileCapError } from '$lib/attachments';
	import { downloadConversationHtml } from '$lib/export';
	import { toasts } from '$lib/toast.svelte';
	import { fontScale, SCALE_MIN, SCALE_MAX } from '$lib/fontscale.svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { qk } from '$lib/queries';
	import AdapterIcon from './AdapterIcon.svelte';
	import MachineBadge from './MachineBadge.svelte';
	import TokenUsage from './TokenUsage.svelte';
	import PermissionCard from './PermissionCard.svelte';
	import AskQuestionCard from './AskQuestionCard.svelte';
	import AttachmentList from './AttachmentList.svelte';

	let {
		session,
		onclose,
		highlight = [],
		onNewFromScript
	}: {
		session: SessionListItem;
		onclose: () => void;
		highlight?: string[];
		// "New session from same script" for archived sessions (CCT-250 item 8).
		onNewFromScript?: (s: SessionListItem) => void;
	} = $props();

	// Search terms to highlight inline (CCT-187), set when opened from a search.
	const hl = (html: string) => (highlight.length ? highlightTerms(html, highlight) : html);

	const id = $derived(session.id);
	const archived = $derived(session.status === 'archived');
	const needsInput = $derived(session.attention === 'needs_input' && !archived);
	const qc = useQueryClient();

	// ── Message-type tag filter (CCT-250 item 2) ──────────────────────────────
	// Each message type is a clickable badge with include/exclude semantics:
	//   'off'      → neutral (shown unless something else is set to 'include')
	//   'include'  → if ANY tag is 'include', only included types render
	//   'exclude'  → always hidden
	// Replaces the old showTool/showMcp/showSystem/showResult booleans.
	type MsgType = 'assistant' | 'user' | 'tool' | 'mcp' | 'system' | 'result';
	type TagState = 'off' | 'include' | 'exclude';
	const MSG_TYPES: { id: MsgType; label: string; role: string }[] = [
		{ id: 'assistant', label: 'Assistant', role: 'assistant' },
		{ id: 'user', label: 'User', role: 'user' },
		{ id: 'tool', label: 'Tools', role: 'tool' },
		{ id: 'mcp', label: 'MCP', role: 'mcp' },
		{ id: 'system', label: 'System', role: 'system' },
		{ id: 'result', label: 'Results', role: 'result' }
	];

	interface ViewOpts {
		// Per-type tag filter state (CCT-250 item 2).
		typeFilter: Record<MsgType, TagState>;
		// Formatting toggles (kept as toggles, visually grouped).
		prettyJson: boolean;
		prettyDiff: boolean;
		prettyTables: boolean;
		// Desktop drawer width in px (drag-to-resize the left border). Null → the
		// default min(900px, 100vw). Persisted with the other view opts.
		paneWidth: number | null;
	}
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
				// typeFilter is nested — merge per-key so a partial/old payload
				// (which had no typeFilter) keeps the sensible defaults.
				typeFilter: { ...defaults.typeFilter, ...(saved.typeFilter ?? {}) }
			};
		} catch {
			return { ...defaults };
		}
	}
	// Cycle a tag: off → include → exclude → off (CCT-250 item 2).
	function cycleTag(t: MsgType) {
		const order: TagState[] = ['off', 'include', 'exclude'];
		const i = order.indexOf(view.typeFilter[t]);
		view.typeFilter = { ...view.typeFilter, [t]: order[(i + 1) % order.length] };
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

	// Live state is kept component-local and fed by ws listener callbacks. This
	// is the only reliable way to re-render: a $derived reading the ws
	// singleton's keyed state from this module did not re-run on mutation.
	let live = $state<AgentEvent[]>([]);
	let perms = $state<PermReq[]>([]);
	// Live AskUserQuestion (CCT-164/179): delivered by the daemon's PreToolUse
	// hook the instant the form renders — before the transcript flushes the full
	// tool call. Carries the structured `questions` (options/multiSelect) so we
	// render the interactive form live, not just text. Null when none pending.
	let ask = $state<LiveAsk | null>(null);
	// Parsed structured questions for the live prompt, or null → text fallback.
	const liveAskQuestions = $derived(ask?.questions ? parseAsk({ questions: ask.questions }) : null);
	// Dedupe the two ask render sites (CCT-218): the hook-driven live card and
	// the transcript-derived `ln.ask` card used to never coexist (the tool_use
	// flushed to the transcript only after answering), but since the daemon
	// holds a headless attach (CCT-209) the transcript line streams in while
	// the question is still pending — showing the same question twice. While a
	// live ask is pending, suppress transcript ask lines that carry the same
	// question; once resolved the live card vanishes and the transcript card
	// remains as history.
	function isDupeOfLiveAsk(a: AskQuestion[]): boolean {
		const q = a[0]?.question;
		if (!q) return false;
		// Already answered/resolved this session (CCT-230): the ask tool_use
		// flushes to the transcript AFTER the answer, so without this the late
		// line pops in below the user's reply as a fresh interactive form (and
		// flickers active once `answering` clears). The reply bubble already
		// carries the Q→A text, so suppress the late duplicate outright.
		if (resolvedAsks.has(askKey(q))) return true;
		if (!ask) return false;
		const liveQ = liveAskQuestions?.[0]?.question;
		if (liveQ !== undefined) return q === liveQ;
		// Text-only fallback delivery: the flattened `question` embeds the text.
		return ask.question.includes(q);
	}
	// Question texts answered (by us) or resolved (by the daemon) this visit,
	// keyed per session so switching sessions can't cross-suppress (CCT-230).
	let resolvedAsks = $state<Set<string>>(new Set());
	const askKey = (q: string) => `${id}|${q}`;
	function markAsksResolved(qs: { question: string }[] | null, fallback?: string) {
		const next = new Set(resolvedAsks);
		for (const q of qs ?? []) next.add(askKey(q.question));
		if (!qs?.length && fallback) next.add(askKey(fallback));
		resolvedAsks = next;
	}
	// The assistant prose preceding the live question (CCT-213), rendered as
	// markdown above the card so the user answers with context, not blind.
	const askPreambleHtml = $derived(ask?.preamble ? hl(renderMarkdown(ask.preamble)) : null);
	// Per-message delivery state (CCT-212), keyed by the optimistic reply's local
	// `ts`. `pendingReplies` = still "sending…" (awaiting the server ack, possibly
	// mid auto-retry); `failedReplies` maps ts → an error reason for sends that
	// exhausted auto-retry — rendered red with a Retry; `retryingReplies` carries
	// the auto-retry progress for a "retrying (n/m)" hint.
	// Local MIRRORS of the ws singleton's per-session delivery state (CCT-214).
	// The source of truth lives on the singleton, so a failed/in-flight send and
	// its auto-retry loop survive the drawer being closed and reopened (the bug:
	// previously these were the source of truth and a full unmount wiped them, so
	// a failed bubble came back plain with no Retry). We mirror into local $state
	// via `ws.onDelivery` — reading the singleton's keyed state from a $derived
	// does NOT re-render (see ws.svelte.ts header).
	let pendingReplies = $state<Set<number>>(new Set());
	let failedReplies = $state<Map<number, string>>(new Map());
	let retryingReplies = $state<Map<number, { attempt: number; max: number }>>(new Map());
	// Activity indicator (CCT-208): true while claude is processing this turn —
	// the equivalent of the TUI's "Running…" spinner, proving the request is
	// being worked on. Set when we send input or see agent/tool/text/reply
	// events stream in; cleared on `turn_end` and whenever claude blocks on the
	// user (a permission prompt or an AskUserQuestion arrives). Stream-derived
	// rather than read off the status poll so it reacts instantly.
	let working = $state(false);
	// Optimistic answer lock (CCT-190): the live ask card and the persisted ask
	// line are two independent render sites; the clicked one may unmount (live
	// card) and be replaced by the other (persisted line) before the server
	// round-trip lands. This session-scoped flag locks BOTH sites to their
	// answered state on click, so the card never pops back to "asking" while the
	// reply is in flight. Cleared on resolution (onAsk), a new ask, or session
	// switch.
	let answering = $state(false);

	// Bumped to force a full re-subscribe + history refetch (e.g. when the tab
	// regains focus after the ws may have gone half-open while backgrounded).
	let resubTick = $state(0);

	// (Re)subscribe + register listeners when the open session changes (or on a
	// forced resubscribe).
	$effect(() => {
		const sid = id;
		void resubTick;
		live = ws.bufferedEvents(sid);
		answering = false;
		working = false;
		histIndex = -1;
		draftStash = '';
		ws.subscribe(sid);
		const offStream = ws.onStream(sid, (ev) => {
			// Skip a server-echoed user message (reply echo or persisted `▷ User:`
			// text) that duplicates our optimistic one already in `live`.
			const key = userMsgKey(ev);
			if (key !== null && live.some((e) => userMsgKey(e) === key)) return;
			live = [...live, ev];
			// Drive the activity indicator (CCT-208): a turn ends on `turn_end`;
			// any substantive agent/tool/user event means work is in progress.
			// Heartbeats keep an active turn alive but never start one.
			if (ev.type === 'turn_end') working = false;
			else if (ev.type !== 'heartbeat') working = true;
		});
		const offPerms = ws.onPerms(sid, (list) => {
			perms = list;
			// A permission prompt means claude is blocked on the user, not working.
			if (list.length) working = false;
		});
		const offAsk = ws.onAsk(sid, (q) => {
			// A pending ask resolving (answered here, from the TUI, or timed out)
			// means its late transcript line must stay suppressed (CCT-230).
			if (!q && ask) markAsksResolved(liveAskQuestions, ask.question);
			ask = q;
			// A fresh ask (or a resolution) supersedes any in-flight answer lock.
			answering = false;
			// A pending question means claude is waiting on the user, not working.
			if (q) working = false;
		});
		// Mirror the singleton's per-session delivery state (CCT-214). Fires
		// immediately with the current snapshot — so reopening the drawer restores
		// a failed send's red+Retry and any in-flight "retrying" tint — and on
		// every subsequent ack / auto-retry transition.
		const offDelivery = ws.onDelivery(sid, (snap) => {
			pendingReplies = snap.pending;
			failedReplies = snap.failed;
			retryingReplies = snap.retrying;
		});
		return () => {
			offStream();
			offPerms();
			offAsk();
			offDelivery();
			ws.unsubscribe(sid);
			ws.clearStream(sid);
		};
	});

	// When the tab is backgrounded the ws can go half-open and miss events; on
	// return we force a fresh history refetch + re-subscribe so the chat catches
	// up automatically (previously the user had to close + reopen the drawer).
	$effect(() => {
		const refresh = () => {
			if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
			ws.connect();
			qc.invalidateQueries({ queryKey: qk.conversation(id) });
			resubTick++;
		};
		const onVis = () => {
			if (document.visibilityState === 'visible') refresh();
		};
		document.addEventListener('visibilitychange', onVis);
		window.addEventListener('focus', refresh);
		return () => {
			document.removeEventListener('visibilitychange', onVis);
			window.removeEventListener('focus', refresh);
		};
	});

	// Content signature of an event, used to dedup the live stream against
	// fetched history (the same logical event has a DIFFERENT `ts` in each
	// source — history stamps DB `created_at`, live carries the daemon ts — so
	// ts can't be the key). User messages collapse across their three shapes via
	// `userMsgKey`. Markers (reset/turn_end/heartbeat) key on ts so distinct ones
	// aren't over-collapsed. NB: we dedup live-against-history only, never
	// live-vs-live, so legitimately-repeated identical tool calls within a turn
	// still each render. Content-based dedup is safe because the server persists
	// with an `ON CONFLICT DO NOTHING` content-hash, so history never holds two.
	function eventSig(e: AgentEvent): string {
		const u = userMsgKey(e);
		if (u !== null) return `u:${u}`;
		switch (e.type) {
			case 'text':
				return `a:${e.content.trim()}`;
			case 'tool_call':
				return `tc:${e.tool}:${JSON.stringify(e.input)}`;
			case 'tool_result':
				return `tr:${e.tool}:${e.output_summary}`;
			case 'compact_summary':
				return `cs:${e.content.trim()}`;
			default:
				return `${e.type}:${e.ts}`;
		}
	}

	// History (fetched) + live (ws) events, merged in order, with live events
	// already present in history dropped so a reconnect/focus refetch (which
	// overlaps the live buffer) and the persisted form of an optimistic reply
	// don't render twice.
	//
	// Ordering (CCT-186 → fixed here): history is monotonic (DB `created_at ASC`)
	// and live events were appended after it. CCT-186 sorted the deduped live
	// *tail* among itself but still pinned it after ALL history (`[...hist,
	// ...tail]`) on the assumption that every surviving live event is newer than
	// history's last row. That assumption breaks for an optimistic reply that
	// survives a focus/reconnect refetch (its persisted form didn't dedup out):
	// it carries an OLDER `ts` than the assistant rows the refetch pulled in, yet
	// the tail-after-history merge stranded it at the very bottom (the "old user
	// message stuck at the bottom" bug). We now order the WHOLE merged list by
	// `ts` with a stable sort, so such an event lands in its correct chronological
	// place. Array.sort is stable, so equal-`ts` ties keep their original order —
	// history (built first) stays ahead of a live event sharing its `ts`.
	const events = $derived.by(() => {
		const hist = $history.data ?? [];
		const seen = new Set(hist.map(eventSig));
		const tail = live.filter((e) => !seen.has(eventSig(e)));
		return [...hist, ...tail].sort((a, b) => a.ts - b.ts);
	});

	interface AskQuestion {
		header?: string;
		question: string;
		multiSelect?: boolean;
		options: { label: string; description?: string; preview?: string }[];
	}
	interface Line {
		role: 'assistant' | 'user' | 'system' | 'tool' | 'result' | 'reset' | 'compact';
		ts: number;
		html?: string;
		// Pre-highlighted code HTML for the <pre> bubble (tool/result), {@html}.
		htmlCode?: string;
		text?: string;
		tool?: string;
		// Tool calls under the mcp__ prefix get the distinct MCP role hue.
		mcp?: boolean;
		pending?: boolean;
		// Set on a pending user line that auto-retry is currently re-attempting
		// (CCT-214): shows a "retrying (n/m)" hint instead of plain "sending…".
		retrying?: { attempt: number; max: number };
		// Set on a user line whose send failed (CCT-212): the error reason, shown
		// red with a Retry control.
		failed?: string;
		// Parsed AskUserQuestion payload (CCT-146) — rendered as interactive cards.
		ask?: AskQuestion[];
	}

	// Pull a well-formed questions[] out of an AskUserQuestion tool input.
	function parseAsk(input: unknown): AskQuestion[] | null {
		const qs = (input as { questions?: unknown })?.questions;
		if (!Array.isArray(qs) || qs.length === 0) return null;
		const out = qs
			.filter((q): q is AskQuestion => !!q && typeof (q as AskQuestion).question === 'string' && Array.isArray((q as AskQuestion).options))
			.map((q) => ({
				header: q.header,
				question: q.question,
				multiSelect: !!q.multiSelect,
				options: q.options.map((o) => ({ label: String(o.label ?? ''), description: o.description, preview: o.preview }))
			}));
		return out.length ? out : null;
	}

	// History stores user turns as a `text` event prefixed with "▷ User:"
	// (there is no `reply` row on read). The marker (imported from ws.svelte) is
	// detected so the user's own messages render as user bubbles instead of
	// blending into assistant text.

	// Some "user" turns are really harness/system messages directed at the
	// agent (timer wake-ups, task-completion notifications, injected reminders)
	// rather than something the human typed. The adapter layer marks these
	// authoritatively via `meta` (Claude's `isMeta` OR known harness tags) — see
	// cctui-daemon transcript parsing — so they render in a distinct hue instead
	// of masquerading as the user's own green bubbles. The tag fallback below
	// only covers events stored before `meta` existed.
	const META_TAGS = ['<task-notification', '<system-reminder', '<command-name', '<command-message', '<local-command', '<bash-input', '<bash-stdout', '<bash-stderr'];
	function looksMeta(text: string): boolean {
		const t = text.trimStart();
		return META_TAGS.some((m) => t.startsWith(m));
	}
	// Render markdown honoring the table formatting toggle (CCT-250 item 2).
	const mdRender = (s: string) => hl(renderMarkdown(s, { tables: view.prettyTables }));
	function userOrSystem(content: string, ts: number, meta: boolean): Line | null {
		const role = meta ? 'system' : 'user';
		if (!typeVisible(role)) return null;
		return { role, ts, html: mdRender(content), text: content };
	}

	function toLine(e: AgentEvent): Line | null {
		switch (e.type) {
			case 'text': {
				// Streaming emits an empty text event before the populated one —
				// skip empties so they don't render as blank assistant blocks.
				if (!e.content.trim()) return null;
				if (e.content.startsWith(USER_PREFIX)) {
					const content = e.content.slice(USER_PREFIX.length).trimStart();
					return userOrSystem(content, Number(e.ts), e.meta || looksMeta(content));
				}
				if (!typeVisible('assistant')) return null;
				return { role: 'assistant', ts: Number(e.ts), html: mdRender(e.content), text: e.content };
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
				const isMcp = e.tool.startsWith('mcp__');
				// MCP tool calls filter on the 'mcp' tag; other tool calls on 'tool'.
				if (!typeVisible(isMcp ? 'mcp' : 'tool')) return null;
				const { text, lang } = formatToolInput(e.tool, e.input);
				return {
					role: 'tool',
					ts: Number(e.ts),
					tool: e.tool,
					mcp: isMcp,
					text,
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
				// Render as a distinct full-width boundary.
				return { role: 'reset', ts: Number(e.ts), text: 'context reset · /clear or /compact' };
			case 'compact_summary':
				// /compact appends a summary in place (no session-id rotation),
				// so it arrives with its text (CCT-159). Render as a distinct
				// "context compacted" block rather than a user bubble.
				if (!e.content.trim()) return null;
				return { role: 'compact', ts: Number(e.ts), html: mdRender(e.content), text: e.content };
			default:
				return null; // heartbeat, turn_end
		}
	}

	function formatToolInput(tool: string, input: unknown): { text: string; lang: string } {
		const obj = input as Record<string, unknown> | null;
		if (view.prettyDiff && obj && typeof obj === 'object' && 'old_string' in obj && 'new_string' in obj) {
			const minus = String(obj.old_string ?? '')
				.split('\n')
				.map((l) => `- ${l}`)
				.join('\n');
			const plus = String(obj.new_string ?? '')
				.split('\n')
				.map((l) => `+ ${l}`)
				.join('\n');
			return { text: `${obj.file_path ?? ''}\n${minus}\n${plus}`.trim(), lang: '' };
		}
		// Shell-ish tools (Bash, BashOutput, …): render the command itself as a
		// shell block with the description as a leading comment, instead of a
		// one-line JSON blob full of literal "\n" escapes — those escapes were the
		// "weird artifacts" / un-prettified commands (CCT-161 cleanup).
		if (view.prettyJson && obj && typeof obj === 'object' && typeof obj.command === 'string') {
			const desc =
				typeof obj.description === 'string' && obj.description.trim()
					? `# ${obj.description.trim()}\n`
					: '';
			return { text: `${desc}${obj.command}`, lang: 'sh' };
		}
		if (!view.prettyJson) return { text: JSON.stringify(input), lang: 'json' };
		// Expand escaped newlines/tabs inside string values so multiline payloads
		// (scripts, file contents, heredocs) read as real lines rather than one
		// long "…\n…" run before the highlighter sees them.
		return { text: expandJsonEscapes(prettyJson(input)), lang: 'json' };
	}

	// JSON.stringify only emits \n / \t inside string literals, so expanding them
	// for display is safe (display-only — the text is never parsed back).
	function expandJsonEscapes(s: string): string {
		return s.replace(/\\n/g, '\n').replace(/\\t/g, '\t');
	}

	// Build lines with consecutive-duplicate dedup.
	const pendingTs = $derived(pendingReplies);
	const failedTs = $derived(failedReplies);
	const retryingTs = $derived(retryingReplies);
	const lines = $derived.by(() => {
		const out: Line[] = [];
		let prevKey = '';
		for (const e of events) {
			const ln = toLine(e);
			if (!ln) continue;
			// Reset markers are keyed by ts so two back-to-back resets aren't
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
		return out;
	});

	// Suppress the live preamble block when the same assistant prose has
	// already streamed into the transcript (CCT-218).
	const preambleInLines = $derived(
		!!ask?.preamble && lines.some((l) => l.role === 'assistant' && (l.text ?? '').trim() === ask!.preamble!.trim())
	);

	const actions = useSessionActions();

	// Composer
	let input = $state(drafts.get(composerKey(session.id)));
	$effect(() => {
		drafts.set(composerKey(session.id), input);
	});

	// ── Mid-chat file attachments (CCT-236) ────────────────────────────────
	// Held client-side next to the draft (File handles can't be persisted to
	// localStorage, so unlike `input` these don't survive a reload — the draft
	// text does). On send we upload first, then append the staged paths under
	// the message text (same convention as spawn-time uploads) so the agent
	// reads them. Only claude-code supports mid-chat staging today.
	const supportsAttachments = $derived(session.adapter_id === 'claude-code');
	let attachments = $state<File[]>([]);
	let uploading = $state(false);
	let dragActive = $state(false);
	const attachError = $derived(fileCapError(attachments));
	const addAttachments = (incoming: File[]) => {
		if (!supportsAttachments || archived) return;
		attachments = mergeFiles(attachments, incoming);
	};
	const removeAttachment = (name: string) => (attachments = removeFileByName(attachments, name));
	function onPickAttachments(e: Event) {
		const el = e.currentTarget as HTMLInputElement;
		addAttachments(Array.from(el.files ?? []));
		el.value = '';
	}

	// ── Message delivery tracking (CCT-212 → CCT-214) ───────────────────────
	// We create the optimistic echo (we own `live` + the `ts` ordering) and hand
	// the send off to the ws singleton, which owns the dispatch + ack timeout +
	// auto-retry-with-backoff loop. Keeping that state on the singleton is what
	// lets a failed/in-flight send survive the drawer being closed and reopened.
	// Returns true if the first frame left the socket (used only for the
	// optimistic working/ask UX — delivery is driven by acks + retries).
	function sendTracked(text: string): boolean {
		const ts = pushOptimisticReply(text);
		return ws.trackedSend(id, text, ts);
	}
	// Re-send a failed message manually (resets the auto-retry counter). The
	// optimistic echo keeps its `ts`, so the bubble stays put.
	function retryFailed(ts: number) {
		if (archived) return;
		working = true;
		stuck = true;
		ws.retryNow(id, ts);
	}

	// ── Cold-cache Send button (CCT-189) ───────────────────────────────────
	// Anthropic's prompt cache is a ~5-min sliding window; once it lapses the
	// next send re-writes the whole context to cache (an expensive "burst").
	// The button's "cold now" is purely time-based: a turn that just completed
	// leaves the cache warm for ~5 min (even a first-turn creation burst), so
	// we flip blue only once the window has elapsed since the last turn — this
	// avoids a false "cold" on a freshly-active session. (The server's
	// backward-looking `cache_cold` flag drives the ❄️ list glyph instead.)
	// A timer re-evaluates so the button flips blue while the drawer sits open.
	const CACHE_TTL_MS = 5 * 60 * 1000;
	// Final-minute countdown window (CCT-261): show a live "Send (Ns)" countdown
	// while the warm cache is within this many ms of going cold.
	const COLD_WARN_MS = 60 * 1000;
	let now = $state(Date.now());
	const lastActivityMs = $derived(
		session.last_activity_at ? new Date(session.last_activity_at).getTime() : null
	);
	const cacheCold = $derived(lastActivityMs !== null && now - lastActivityMs > CACHE_TTL_MS);
	const burstTokens = $derived(session.estimated_burst_tokens ?? null);
	// Milliseconds until the warm window lapses (null when no activity / cold).
	const msUntilCold = $derived(
		lastActivityMs === null ? null : CACHE_TTL_MS - (now - lastActivityMs)
	);
	// Whether we're in the final-minute countdown band (warm, but ≤60s left).
	const coldImminent = $derived(msUntilCold !== null && msUntilCold > 0 && msUntilCold <= COLD_WARN_MS);
	// Seconds to display, clamped to [0, 60].
	const coldCountdownSecs = $derived(coldImminent ? Math.ceil(msUntilCold! / 1000) : null);
	// Tick fast (1s) only while counting down so the number is smooth; otherwise
	// a lazy 15s tick is enough to flip the button cold. Re-evaluates as the
	// session (last_activity_at) changes; the interval is torn down on unmount.
	$effect(() => {
		const fast = coldImminent;
		const t = setInterval(() => (now = Date.now()), fast ? 1_000 : 15_000);
		return () => clearInterval(t);
	});

	// ── Sent-message history recall (ArrowUp/ArrowDown) ─────────────────────
	// histIndex: -1 = editing the live draft; 0..n-1 = browsing history
	// (newest-first as you press Up). draftStash holds the in-progress text so
	// returning past the newest entry restores it.
	let histIndex = $state(-1);
	let draftStash = '';
	function resetHistoryNav() {
		histIndex = -1;
	}

	async function send() {
		const text = input.trim();
		// Allow sending attachments with no text (the staged paths become the
		// message), but require at least one of text/attachments.
		if ((!text && attachments.length === 0) || archived || uploading) return;
		if (attachError) {
			toasts.err(attachError);
			return;
		}

		// Stage any pending attachments first; append the staged absolute paths
		// under the message so the agent reads them (CCT-236). On failure keep the
		// draft + attachments intact and surface the error rather than sending a
		// half-message.
		let body = text;
		if (attachments.length) {
			uploading = true;
			try {
				const { paths } = await actions.stageFiles(id, attachments);
				const list = paths.map((p) => `- ${p}`).join('\n');
				const header =
					paths.length === 1 ? 'Attached file:' : `Attached files (${paths.length}):`;
				body = text ? `${text}\n\n${header}\n${list}` : `${header}\n${list}`;
				attachments = [];
			} catch (e) {
				toasts.err(`Attachment upload failed: ${(e as Error).message}`);
				return;
			} finally {
				uploading = false;
			}
		}
		sendBody(body);
	}

	// The original send path, now operating on the final message body (text +
	// any appended staged-attachment paths).
	function sendBody(text: string) {
		if (!text || archived) return;
		// NB: a free-typed send does NOT dismiss a pending AskUserQuestion
		// (CCT-208). The old code cleared `ask` on every send, which hid the
		// question whenever the user had a message in flight — but the question
		// is still genuinely pending until the daemon emits `ask_resolved`
		// (which arrives a poll later and clears it via onAsk). Only an explicit
		// option-click (`answerQuestion`) optimistically dismisses the card.
		// Sending should always jump to the latest message (classic chat UX),
		// even if the user had scrolled up — re-pin so the sticky-bottom $effect
		// follows the optimistic echo down.
		stuck = true;
		// Optimistic echo with delivery tracking (CCT-212): the bubble shows
		// "sending…" until the server acks, then goes red+Retry on failure. A
		// dropped frame no longer vanishes silently. We're now processing the
		// user's input, so flag the working indicator only if the frame went out.
		const ok = sendTracked(text);
		if (ok) working = true;
		msgHistory.push(session.id, text);
		input = '';
		resetHistoryNav();
		drafts.clear(composerKey(session.id));
		// Reflect the new turn in the list (last-message / ordering) without
		// waiting for the next poll.
		qc.invalidateQueries({ queryKey: ['sessions'] });
	}
	// On touch/mobile, a bare Enter should insert a newline (the on-screen
	// keyboard's return key is easy to hit by accident) — send only via the
	// Send button or Ctrl/Cmd+Enter. On desktop, Enter still sends and
	// Shift+Enter inserts a newline.
	const coarsePointer =
		typeof window !== 'undefined' &&
		typeof window.matchMedia === 'function' &&
		window.matchMedia('(pointer: coarse)').matches;
	let textarea = $state<HTMLTextAreaElement>();

	// True when the caret is at the very start of the textarea (so ArrowUp can
	// recall history without fighting normal multiline cursor movement). Up only
	// recalls from the first line; Down only advances when on the last line.
	function caretAtStart(): boolean {
		const el = textarea;
		if (!el) return false;
		return el.selectionStart === 0 && el.selectionEnd === 0;
	}
	function caretAtEnd(): boolean {
		const el = textarea;
		if (!el) return false;
		return el.selectionStart === input.length && el.selectionEnd === input.length;
	}

	function historyBack() {
		const list = msgHistory.get(session.id);
		if (list.length === 0) return;
		if (histIndex === -1) draftStash = input; // stash live draft before browsing
		const next = Math.min(histIndex + 1, list.length - 1);
		histIndex = next;
		input = list[list.length - 1 - next]; // newest-first
	}
	function historyForward() {
		const list = msgHistory.get(session.id);
		if (histIndex === -1) return;
		const next = histIndex - 1;
		if (next < 0) {
			histIndex = -1;
			input = draftStash; // restored the in-progress draft
		} else {
			histIndex = next;
			input = list[list.length - 1 - next];
		}
	}

	function onKey(e: KeyboardEvent) {
		if (e.key === 'ArrowUp' && (histIndex !== -1 || caretAtStart())) {
			e.preventDefault();
			historyBack();
			return;
		}
		if (e.key === 'ArrowDown' && histIndex !== -1 && caretAtEnd()) {
			e.preventDefault();
			historyForward();
			return;
		}
		if (e.key !== 'Enter') return;
		if (e.ctrlKey || e.metaKey) {
			e.preventDefault();
			send();
			return;
		}
		if (!coarsePointer && !e.shiftKey) {
			e.preventDefault();
			send();
		}
	}

	// Answer an AskUserQuestion (CCT-146). With pure option picks the daemon
	// drives the real form via PTY keystrokes (CCT-226) so claude records a
	// genuine tool_result with the selected labels; the flattened text rides
	// along as the carrier for the free-text/fallback path (dismiss the form,
	// reply the text — which claude records as a declined ask + user turn).
	function answerQuestion(text: string, picks: number[][] | null, qs?: AskQuestion[] | null) {
		if (archived) return;
		// Track delivery like any reply (CCT-212). If the frame can't go out, the
		// optimistic bubble shows failed+Retry and we keep the question on screen
		// rather than dismissing it for a send that never left.
		const ts = pushOptimisticReply(text);
		const ok = ws.trackedSend(id, text, ts, picks ?? undefined);
		if (!ok) return;
		// Lock both ask render sites to their answered state immediately (CCT-190),
		// and remember the answered questions so the late-flushing transcript line
		// never resurfaces as a fresh form (CCT-230).
		markAsksResolved(qs ?? liveAskQuestions, ask?.question);
		answering = true;
		// Answering hands control back to claude — show the working indicator.
		working = true;
		// Dismiss the live prompt immediately — the daemon's AskResolved arrives
		// a poll later (CCT-164).
		ask = null;
		ws.clearAsk(id);
		qc.invalidateQueries({ queryKey: ['sessions'] });
	}

	// Edit a still-pending message (CCT-208). A pending optimistic reply may not
	// have been delivered (e.g. the daemon was offline), so let the user pull it
	// back into the composer to fix and resend: drop the echo from both the local
	// buffer and the ws optimistic store, clear its pending tint, and focus the
	// textarea with the recovered text.
	function editPending(text: string, ts: number) {
		if (archived) return;
		input = text;
		// Stop tracking/retrying this send and drop its echo from both the local
		// buffer and the ws optimistic store (CCT-214 / CCT-208).
		ws.cancelSend(id, ts);
		ws.dropOptimistic(id, ts);
		live = live.filter((e) => !(e.type === 'reply' && e.ts === ts));
		resetHistoryNav();
		textarea?.focus();
	}

	// Optimistic echo of a user-typed message. Kept in the ws singleton (not
	// just local `live`) so a resubscribe/reconnect that rebuilds `live` from
	// `bufferedEvents()` doesn't drop a message claude already received.
	function pushOptimisticReply(text: string): number {
		// Stamp the optimistic echo just past the newest known event rather than
		// with the browser clock (CCT-186): a user message is logically the latest
		// thing in the conversation at send time, but `Date.now()` is a third clock
		// that can sit behind the daemon-stamped `ts` of surrounding events and so
		// sort the message into the wrong place. Deriving `ts` from the current max
		// keeps it ordered last until history catches up and replaces it.
		const known = [...($history.data ?? []), ...live];
		const maxTs = known.reduce((m, e) => Math.max(m, e.ts), 0);
		const ts = Math.max(Date.now(), maxTs + 1);
		const ev: AgentEvent = { type: 'reply', content: text, ts };
		ws.recordOptimistic(id, ev);
		live = [...live, ev];
		// `pendingReplies` is no longer set here — the caller hands this `ts` to
		// `ws.trackedSend`, which marks it pending and pushes that via onDelivery
		// (CCT-214). Single source of truth on the singleton.
		return ts;
	}

	async function copyLine(text: string) {
		try {
			await navigator.clipboard.writeText(text);
			toasts.ok('Copied');
		} catch {
			toasts.err('Clipboard unavailable');
		}
	}

	let renaming = $state(false);
	let newName = $state(session.name ?? '');
	async function doRename() {
		const n = newName.trim();
		renaming = false;
		if (!n) return;
		try {
			await actions.rename(id, n);
			toasts.ok('Renamed');
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	async function doArchive() {
		try {
			await actions.archive(id);
			// Wipe this session's local composer state (draft + sent-message
			// history) — it's gone once archived (CCT-162) — and stop tracking any
			// in-flight/failed sends so auto-retry doesn't run on a dead session.
			clearSessionStorage(session.id);
			ws.clearDelivery(session.id);
			toasts.ok('Archived');
			onclose();
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	// Replaces the misleading "unarchive" (CCT-250 item 8): the agent-side worker
	// is gone, so re-dispatch a fresh session seeded with this one's config.
	function newFromScript() {
		onNewFromScript?.(session);
	}

	// Export the transcript as a self-contained HTML file (CCT-227). Built from
	// `events` (history + live) but gated by the current view toggles and themed
	// with the active palette — the export matches what's on screen.
	function doExport() {
		try {
			downloadConversationHtml(session, events, view);
			toasts.ok('Transcript downloaded');
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	// Copy the session's stable, shareable URL (CCT-206) so it can be pasted into
	// a PR/comment. Same-origin link gated by the login wall — only authed people
	// who follow it can read the log.
	async function doCopyLink() {
		const url = `${location.origin}/sessions?session=${id}`;
		try {
			await navigator.clipboard.writeText(url);
			toasts.ok('Link copied');
		} catch {
			toasts.err(url);
		}
	}

	async function doInterrupt() {
		try {
			await actions.interrupt(id);
			toasts.ok('Interrupted');
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	async function toggleAutoApprove() {
		const want = !session.auto_approve;
		try {
			await actions.setAutoApprove(id, want);
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	const headTitle = $derived(session.name || session.working_dir);

	// ── Sticky-bottom scroll (CCT-161 item 7) ──────────────────────────────
	// If the user is at the bottom, auto-scroll with new content (sticky). If
	// scrolled up, don't yank them down — show a "jump to bottom" pill instead.
	let scroller = $state<HTMLElement>();
	let stuck = $state(true); // currently pinned to the bottom
	const STICK_SLOP = 48; // px from bottom still counts as "at bottom"

	function atBottom(): boolean {
		const el = scroller;
		if (!el) return true;
		return el.scrollHeight - el.scrollTop - el.clientHeight <= STICK_SLOP;
	}
	function onScroll() {
		stuck = atBottom();
	}
	function jumpToBottom() {
		if (scroller) scroller.scrollTop = scroller.scrollHeight;
		stuck = true;
	}
	$effect(() => {
		void lines.length;
		void perms.length;
		void working;
		// Only follow new content when the user is pinned to the bottom.
		if (stuck && scroller) {
			requestAnimationFrame(() => {
				if (scroller) scroller.scrollTop = scroller.scrollHeight;
			});
		}
	});
	// Reset to bottom + sticky when switching sessions.
	$effect(() => {
		void id;
		stuck = true;
	});

	// Keep pinned to the bottom while the composer grows (CCT-161). When the user
	// is at the bottom and types a long message, the auto-resizing textarea steals
	// vertical space from the chat; without this the latest lines scroll out of
	// view. Observe the textarea and re-pin on each resize while stuck.
	$effect(() => {
		const el = textarea;
		if (!el || typeof ResizeObserver === 'undefined') return;
		const ro = new ResizeObserver(() => {
			if (stuck && scroller) scroller.scrollTop = scroller.scrollHeight;
		});
		ro.observe(el);
		return () => ro.disconnect();
	});

	// ── Drag-to-resize the desktop drawer (left border) ─────────────────────
	let resizing = $state(false);
	function startResize(e: PointerEvent) {
		resizing = true;
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		e.preventDefault();
	}
	function onResize(e: PointerEvent) {
		if (!resizing) return;
		const w = window.innerWidth - e.clientX;
		view.paneWidth = Math.round(Math.max(PANE_MIN, Math.min(w, window.innerWidth)));
	}
	function endResize(e: PointerEvent) {
		if (!resizing) return;
		resizing = false;
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

<svelte:window onkeydown={(e) => e.key === 'Escape' && !renaming && onclose()} />

<!-- Desktop side-pane: a scrim over the rest of the viewport so clicking
     outside the pane (or Escape) closes it, instead of hunting for the ‹ icon.
     Hidden on mobile where the drawer is full-width. -->
<div
	class="scrim"
	role="button"
	tabindex="-1"
	aria-label="Close conversation"
	onclick={onclose}
	onkeydown={(e) => e.key === 'Escape' && onclose()}
></div>

<div
	class="drawer"
	class:resizing
	style="--drawer-width: {drawerWidth}"
>
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
	<div class="dhead">
		<div class="hrow">
			<button class="tapbtn back" aria-label="Back" onclick={onclose}>‹</button>
			<AdapterIcon adapter={session.adapter_id} size={20} />
			<div class="dtitle">
				{#if renaming}
					<input
						class="input"
						bind:value={newName}
						onkeydown={(e) => e.key === 'Enter' && doRename()}
					/>
				{:else}
					<span class="truncate name">{headTitle}</span>
				{/if}
			</div>
			{#if renaming}
				<button class="tapbtn" aria-label="Save" onclick={doRename}>✓</button>
			{:else}
				<button
					class="tapbtn"
					aria-label="Rename"
					onclick={() => {
						renaming = true;
						newName = session.name ?? '';
					}}>✎</button
				>
			{/if}
			<button
				class="tapbtn"
				aria-label="Copy shareable link"
				title="Copy a stable link to this session (paste in a PR — login-gated)"
				onclick={doCopyLink}
			>
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
					<path d="M10 13a5 5 0 0 0 7.07 0l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
					<path d="M14 11a5 5 0 0 0-7.07 0l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
				</svg>
			</button>
			<button
				class="tapbtn"
				aria-label="Export conversation"
				title="Download transcript as HTML (print it for a PDF)"
				onclick={doExport}
			>
				<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
					<path d="M12 3v12" />
					<path d="m7 10 5 5 5-5" />
					<path d="M4 19h16" />
				</svg>
			</button>
			{#if !archived}
				<button class="tapbtn interrupt" aria-label="Interrupt turn" title="Interrupt the in-flight turn" onclick={doInterrupt}>
					<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
						<rect x="6" y="6" width="12" height="12" rx="1.5" />
					</svg>
				</button>
				<button class="tapbtn archive" aria-label="Archive" onclick={doArchive}>
					<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
						<rect x="3" y="4" width="18" height="4" rx="1" />
						<path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8" />
						<path d="M9 12h6" />
					</svg>
				</button>
			{/if}
		</div>
		<div class="hmeta row row-wrap">
			<span class="badge {statusBadgeClass(session.status)}">{session.status}</span>
			{#if session.model}<span class="chip">{session.model}{session.effort ? ` · ${session.effort}` : ''}</span>{/if}
			<MachineBadge name={session.machine_name} id={session.machine_id} hue={session.machine_hue} mono />
			<button
				class="chip mono cwd truncate"
				title="Click to copy — {session.working_dir}"
				onclick={() => copyLine(session.working_dir)}
			>📁 {session.working_dir} ⧉</button>
			<TokenUsage usage={session.token_usage} />
		</div>
	</div>

	<div class="toolbar">
		<!-- Message-type filters: click a tag to cycle off → include → exclude.
		     Active (include) tags wear their message-badge color; excluded tags
		     show a strike. (CCT-250 item 2) -->
		<div class="tagbar row row-wrap" role="group" aria-label="Message type filter">
			{#each MSG_TYPES as t (t.id)}
				<button
					type="button"
					class="tag {t.id}"
					class:include={view.typeFilter[t.id] === 'include'}
					class:exclude={view.typeFilter[t.id] === 'exclude'}
					title={`${t.label}: ${view.typeFilter[t.id] === 'include' ? 'only this' : view.typeFilter[t.id] === 'exclude' ? 'hidden' : 'shown'} — click to cycle`}
					aria-pressed={view.typeFilter[t.id] !== 'off'}
					onclick={() => cycleTag(t.id)}
				>
					{#if view.typeFilter[t.id] === 'exclude'}✕ {/if}{t.label}
				</button>
			{/each}
		</div>
		<!-- Formatting toggles: gray when off, colored when on. -->
		<div class="fmtbar row row-wrap" role="group" aria-label="Formatting">
			<button type="button" class="fmt" class:on={view.prettyJson} aria-pressed={view.prettyJson} onclick={() => (view.prettyJson = !view.prettyJson)}>JSON</button>
			<button type="button" class="fmt" class:on={view.prettyDiff} aria-pressed={view.prettyDiff} onclick={() => (view.prettyDiff = !view.prettyDiff)}>Diff</button>
			<button type="button" class="fmt" class:on={view.prettyTables} aria-pressed={view.prettyTables} onclick={() => (view.prettyTables = !view.prettyTables)} title="Render markdown tables as tables">Tables</button>
			<!-- Second control for the SINGLE global UI scale (CCT-265): same
			     setting the header slider drives, exposed here so it's reachable
			     while the conversation is open. Both write fontScale. -->
			<label class="tg font" title="UI font size">
				<span aria-hidden="true">A</span>
				<input
					type="range"
					min={SCALE_MIN}
					max={SCALE_MAX}
					step="0.05"
					value={fontScale.current}
					oninput={(e) => fontScale.set(Number((e.currentTarget as HTMLInputElement).value))}
					aria-label="UI font size"
				/>
			</label>
		</div>
		<!-- Behavior toggle: distinct from filters/formatting. -->
		<div class="behbar row row-wrap" role="group" aria-label="Behavior">
			<button
				type="button"
				class="beh"
				class:on={session.auto_approve}
				aria-pressed={session.auto_approve}
				title="Auto-approve permission requests for this session"
				onclick={toggleAutoApprove}
			>⚡ Auto-approve</button>
		</div>
	</div>

	{#if needsInput}
		<div class="attn-banner">✋ Waiting for your input</div>
	{/if}

	<div
		class="conv-wrap"
		use:dropzone={{
			onFiles: addAttachments,
			onActive: (a) => (dragActive = a),
			disabled: !supportsAttachments || archived
		}}
	>
	<div
		class="conv"
		bind:this={scroller}
		onscroll={onScroll}
	>
		{#if $history.isLoading}
			<div class="empty"><span class="spin"></span></div>
		{:else if lines.length === 0 && perms.length === 0 && !ask}
			<div class="empty">No events yet.</div>
		{/if}

		{#each lines as ln, i (ln.ts + (ln.text ?? ln.html ?? '').slice(0, 24) + ln.role)}
			{#if ln.ask && isDupeOfLiveAsk(ln.ask)}
				<!-- Suppressed: same question is rendered live below (CCT-218). -->
			{:else if ln.ask}
				<AskQuestionCard
					questions={ln.ask}
					interactive={i === lines.length - 1 && !archived && !answering && !ask}
					onsubmit={(t, p) => answerQuestion(t, p, ln.ask)}
				/>
			{:else if ln.role === 'reset'}
				<div class="reset-divider" role="separator">
					<span class="reset-chip">⟳ {ln.text}</span>
				</div>
			{:else if ln.role === 'compact'}
				<div class="compact-block">
					<div class="compact-head">⟳ context compacted · /compact</div>
					{#if ln.html}<div class="compact-body">{@html ln.html}</div>{/if}
				</div>
			{:else}
			<div
				class="line {ln.role}"
				class:mcp={ln.mcp}
				class:pending={ln.pending}
				class:failed={!!ln.failed}
			>
				<div class="lmeta row">
					<span class="badge-role" class:mcp={ln.mcp}
						>{ln.mcp ? 'mcp' : ln.role === 'result' ? 'result' : ln.role}</span
					>
					{#if ln.role === 'tool' || ln.role === 'result'}
						<span class="who tool-name">{ln.role === 'result' ? '↳ ' : ''}{ln.tool ?? 'tool'}</span>
					{/if}
					<span class="faint sm">{clockTime(ln.ts)}</span>
					{#if ln.failed}
						<span class="sm not-delivered" title={ln.failed}>⚠ Not delivered</span>
						{#if !archived}
							<button
								class="btn btn-ghost retry-failed"
								aria-label="Retry sending this message"
								title="Resend this message ({ln.failed})"
								onclick={() => retryFailed(ln.ts)}>↻ Retry</button
							>
							<button
								class="btn btn-ghost edit-pending"
								aria-label="Edit message"
								title="Pull this message back into the composer to edit and resend"
								onclick={() => editPending(ln.text ?? '', ln.ts)}>✎</button
							>
						{/if}
					{:else if ln.pending}
						{#if ln.retrying}
							<span class="faint sm sending" title="Delivery failed — retrying with backoff"
								>retrying… ({ln.retrying.attempt}/{ln.retrying.max})</span
							>
						{:else}
							<span class="faint sm sending">sending…</span>
						{/if}
						{#if !archived}
							<button
								class="btn btn-ghost edit-pending"
								aria-label="Edit pending message"
								title="Pull this still-pending message back into the composer to edit and resend"
								onclick={() => editPending(ln.text ?? '', ln.ts)}>✎</button
							>
						{/if}
					{/if}
					<button class="btn btn-ghost copy" aria-label="Copy" onclick={() => copyLine(ln.text ?? '')}>⧉</button>
				</div>
				{#if ln.html}
					<div class="bubble">{@html ln.html}</div>
				{:else if ln.htmlCode}
					<pre class="bubble mono code">{@html ln.htmlCode}</pre>
				{:else}
					<pre class="bubble mono code">{ln.text}</pre>
				{/if}
			</div>
			{/if}
		{/each}

		{#if ask}
			<!-- Live AskUserQuestion (CCT-181): the daemon's hook forwards the
			     structured options, so render the interactive option-card form
			     live. Older deliveries (no structured payload) fall back to the
			     question text with a free-text answer. Answering sends a reply. -->
			{#if askPreambleHtml && !preambleInLines}
				<!-- The assistant prose preceding the question (CCT-213): the
				     reasoning the choice depends on, so the user isn't blind. -->
				<div class="line assistant ask-preamble">
					<div class="bubble">{@html askPreambleHtml}</div>
				</div>
			{/if}
			<AskQuestionCard
				questions={liveAskQuestions ?? [{ question: ask.question, options: [] }]}
				interactive={!archived && !answering}
				onsubmit={(t, p) => answerQuestion(t, p, liveAskQuestions)}
			/>
		{/if}

		{#each perms as p (p.request_id)}
			<PermissionCard req={p} onrespond={(rid, allow) => ws.respondPermission(id, rid, allow)} />
		{/each}

		{#if working && !archived && !ask && perms.length === 0}
			<!-- Activity indicator (CCT-208): proves the request is being processed,
			     the equivalent of the TUI's "Running…" spinner. -->
			<div class="working" role="status" aria-live="polite">
				<span class="working-dots" aria-hidden="true"><span></span><span></span><span></span></span>
				<span class="working-label">Working…</span>
			</div>
		{/if}
	</div>

		{#if !stuck}
			<button class="jump-pill" onclick={jumpToBottom} aria-label="Jump to bottom">
				↓ Jump to latest
			</button>
		{/if}
	</div>

	<div class="composer" class:dropping={dragActive}>
		{#if archived}
			<div class="hint muted">
				Session archived (read-only). The worker is gone on the agent side —
				<button type="button" class="link" onclick={newFromScript}>start a new session from the same script</button>
				to continue.
			</div>
		{:else}
			<!-- Failed sends now surface inline on the message bubble itself
			     (red + Retry, CCT-212), so there's no separate composer banner. -->
			{#if supportsAttachments && attachments.length}
				<div class="attachments">
					<AttachmentList files={attachments} onremove={removeAttachment} compact />
				</div>
			{/if}
			<div class="composer-row">
				{#if supportsAttachments}
					<!-- File picker (CCT-236). Drag-and-drop onto the conversation pane
					     also adds attachments. -->
					<label class="attach-btn" title="Attach files">
						📎
						<input
							class="file-hidden"
							type="file"
							multiple
							onchange={onPickAttachments}
						/>
					</label>
				{/if}
				<textarea
					class="textarea grow"
					rows="1"
					placeholder={dragActive
						? 'Drop files to attach'
						: coarsePointer
							? 'Message…'
							: 'Message… (Enter to send)'}
					bind:value={input}
					bind:this={textarea}
					onkeydown={onKey}
					oninput={() => resetHistoryNav()}
					use:autoresize={input}
				></textarea>
				<button
					class="btn btn-primary send"
					class:cold={cacheCold}
					class:warning={coldImminent}
					disabled={uploading || (!input.trim() && attachments.length === 0)}
					onclick={send}
				title={cacheCold
					? burstTokens
						? `Prompt cache is cold — the next send re-writes ~${compact(burstTokens)} tokens to cache`
						: 'Prompt cache is cold — the next send re-bills the full context'
					: coldImminent
						? 'Prompt cache goes cold soon — send now to keep it warm'
						: undefined}
			>
				{#if uploading}Uploading…{:else if coldImminent}Send (<span class="countdown">{coldCountdownSecs}s</span>){:else if cacheCold && burstTokens}Send ❄️ ~{compact(burstTokens)}{:else if cacheCold}Send ❄️{:else}Send{/if}
			</button>
			</div>
		{/if}
	</div>
</div>

<style>
	/* No scrim on mobile — the drawer is full-width, nothing behind to click. */
	.scrim {
		display: none;
	}
	@media (min-width: 960px) {
		.scrim {
			display: block;
			position: fixed;
			inset: 0;
			z-index: var(--z-drawer);
			background: rgba(0, 0, 0, 0.35);
			animation: fade 0.18s var(--ease);
		}
	}
	@keyframes fade {
		from {
			opacity: 0;
		}
	}
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
	/* Full-width on narrow viewports; a right-anchored side pane on wide ones
	   (matching the HTML-only version's min(900px,100vw) drawer). */
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
		.resize-handle::after {
			content: '';
			position: absolute;
			inset: 0 auto 0 5px;
			width: 1px;
			background: transparent;
			transition: background 0.12s var(--ease);
		}
		.resize-handle:hover::after,
		.drawer.resizing .resize-handle::after {
			background: var(--accent);
			box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent) 40%, transparent);
		}
	}
	@keyframes slide {
		from {
			transform: translateX(4%);
			opacity: 0.5;
		}
	}
	.dhead {
		position: sticky;
		top: 0;
		z-index: 2;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
		background: var(--bg-elevated);
	}
	.hrow {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.dtitle {
		flex: 1;
		min-width: 0;
	}
	.name {
		font-weight: var(--fw-semibold);
		font-size: var(--fs-md);
	}
	/* Bigger, easy-to-tap icon buttons with a tinted, outlined chip look. */
	.tapbtn {
		flex: none;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 2.5rem;
		height: 2.5rem;
		font-size: 1.35rem;
		line-height: 1;
		border-radius: var(--r-md);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border-strong);
		color: var(--text);
	}
	.tapbtn.back {
		font-size: 1.8rem;
	}
	.tapbtn.archive {
		color: var(--warn);
		border-color: color-mix(in srgb, var(--warn) 40%, var(--border-strong));
		background: color-mix(in srgb, var(--warn) 10%, var(--bg-elevated-2));
	}
	.tapbtn.interrupt {
		color: var(--danger, #bf616a);
		border-color: color-mix(in srgb, var(--danger, #bf616a) 40%, var(--border-strong));
		background: color-mix(in srgb, var(--danger, #bf616a) 10%, var(--bg-elevated-2));
	}
	.hmeta {
		gap: var(--sp-2);
	}
	.chip {
		font-size: var(--fs-xs);
		color: var(--text-muted);
		background: var(--bg-elevated-2);
		border: 1px solid var(--border);
		border-radius: var(--r-pill);
		padding: 0.1rem var(--sp-2);
		max-width: 100%;
	}
	/* cwd is a click-to-copy button styled as a chip. */
	button.chip {
		cursor: pointer;
		font-family: var(--font-mono);
	}
	button.chip:hover {
		border-color: var(--border-strong);
		color: var(--text);
	}
	.chip.cwd {
		flex: 1;
		min-width: 6rem;
		text-align: left;
	}
	.sm {
		font-size: var(--fs-xs);
	}
	/* Toolbar (CCT-250 item 2): three visually-separated groups — message-type
	   tag filter, formatting toggles, behavior toggle — divided by thin rules. */
	.toolbar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
		overflow-x: auto;
		font-size: var(--fs-xs);
	}
	.tagbar,
	.fmtbar,
	.behbar {
		gap: var(--sp-1);
	}
	.fmtbar,
	.behbar {
		padding-left: var(--sp-3);
		border-left: 1px solid var(--border);
	}
	/* Message-type tag badge. Gray when neutral; takes its role color when
	   active (include); struck-through danger tint when excluded. */
	.tag {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 0.15rem var(--sp-2);
		border-radius: var(--r-pill);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		line-height: 1.4;
		white-space: nowrap;
		background: var(--bg-elevated-2);
		color: var(--text-muted);
		border: 1px solid var(--border);
		cursor: pointer;
	}
	.tag:hover {
		border-color: var(--border-strong);
	}
	.tag.assistant {
		--tc: var(--role-assistant);
	}
	.tag.user {
		--tc: var(--role-user);
	}
	.tag.tool {
		--tc: var(--role-tool);
	}
	.tag.mcp {
		--tc: var(--role-mcp);
	}
	.tag.system {
		--tc: var(--role-system);
	}
	.tag.result {
		--tc: var(--role-tool);
	}
	.tag.include {
		color: var(--tc);
		border-color: color-mix(in srgb, var(--tc) 55%, transparent);
		background: color-mix(in srgb, var(--tc) 16%, transparent);
	}
	.tag.exclude {
		color: var(--danger);
		border-color: color-mix(in srgb, var(--danger) 45%, transparent);
		background: color-mix(in srgb, var(--danger) 12%, transparent);
		text-decoration: line-through;
	}
	/* Formatting toggle button: gray when off, accent-colored when on. */
	.fmt,
	.beh {
		padding: 0.15rem var(--sp-2);
		border-radius: var(--r-sm);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		white-space: nowrap;
		background: var(--bg-elevated-2);
		color: var(--text-muted);
		border: 1px solid var(--border);
		cursor: pointer;
	}
	.fmt.on {
		color: var(--accent);
		border-color: color-mix(in srgb, var(--accent) 55%, transparent);
		background: color-mix(in srgb, var(--accent) 14%, transparent);
	}
	/* Behavior toggle visually distinct (warm/amber) from filters + formatting. */
	.beh.on {
		color: var(--warn);
		border-color: color-mix(in srgb, var(--warn) 55%, transparent);
		background: color-mix(in srgb, var(--warn) 14%, transparent);
	}
	.tg {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		white-space: nowrap;
		color: var(--text-muted);
	}
	/* Positioning context for the jump-pill so it anchors to the bottom of the
	   chat display area, never overlapping the (growable) composer (CCT-161). */
	.conv-wrap {
		position: relative;
		flex: 1;
		display: flex;
		flex-direction: column;
		min-height: 0;
		/* CCT-172: keep vertical scroll native; we handle horizontal swipes. */
		touch-action: pan-y;
	}
	.conv {
		flex: 1;
		overflow-y: auto;
		/* Keep the chat's scroll inside the pane (CCT-241): without this,
		   hitting the top/bottom of a long log chains the swipe to the page
		   behind, scrolling the app under the drawer. */
		overscroll-behavior: contain;
		-webkit-overflow-scrolling: touch;
		padding: var(--sp-3);
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.line {
		display: flex;
		flex-direction: column;
		gap: 2px;
		max-width: 100%;
	}
	.lmeta {
		gap: var(--sp-2);
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.who {
		text-transform: uppercase;
		letter-spacing: 0.04em;
		font-weight: var(--fw-medium);
	}
	.tool-name {
		font-family: var(--font-mono);
		color: var(--text-muted);
		text-transform: none;
		letter-spacing: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 60%;
	}
	/* Role badge pill (CCT-161 item 2) — colored per role via --role-* tokens. */
	.badge-role {
		--bc: var(--text-muted);
		display: inline-flex;
		align-items: center;
		padding: 1px var(--sp-2);
		border-radius: var(--r-pill);
		font-size: 0.6875rem;
		font-weight: var(--fw-semibold);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--bc);
		background: color-mix(in srgb, var(--bc) 14%, transparent);
		border: 1px solid color-mix(in srgb, var(--bc) 40%, transparent);
		white-space: nowrap;
	}
	.line.user .badge-role {
		--bc: var(--role-user);
	}
	.line.assistant .badge-role {
		--bc: var(--role-assistant);
	}
	.line.system .badge-role {
		--bc: var(--role-system);
	}
	.line.tool .badge-role {
		--bc: var(--role-tool);
	}
	.line.result .badge-role {
		--bc: var(--role-tool);
	}
	.line.tool.mcp .badge-role,
	.badge-role.mcp {
		--bc: var(--role-mcp);
	}
	.copy {
		margin-left: auto;
		padding: 0 var(--sp-2);
		min-height: auto;
		font-size: var(--fs-lg);
		line-height: 1;
	}
	.bubble {
		padding: var(--sp-2) var(--sp-3);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		border: 1px solid var(--border);
		overflow-wrap: anywhere;
		word-break: break-word;
		/* CCT-161 item 4 — slider-driven, falls back to --fs-sm. */
		font-size: var(--fs-sm);
	}
	/* Uniform role tints (CCT-161 item 1) — all via --role-* tokens. */
	.line.user .bubble {
		background: color-mix(in srgb, var(--role-user) 14%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-user) 45%, transparent);
	}
	.line.assistant .bubble {
		border-left: 2px solid color-mix(in srgb, var(--role-assistant) 55%, transparent);
	}
	/* System/agent-directed messages (harness wake-ups, task notifications,
	   injected reminders) — purple, distinct from the green user bubbles so
	   they don't read as something the human typed. */
	.line.system .bubble {
		background: color-mix(in srgb, var(--role-system) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--role-system) 40%, transparent);
	}
	/* Optimistic reply: muted/amber until the agent acknowledges, then it
	   settles into the regular green user tint above. */
	.line.user.pending .bubble {
		background: color-mix(in srgb, var(--warn) 10%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--warn) 35%, transparent);
		opacity: 0.85;
	}
	.sending {
		color: var(--warn);
		margin-left: auto;
	}
	/* Edit-pending button (CCT-208): sits next to "sending…" on a pending line. */
	.edit-pending {
		padding: 0 var(--sp-1);
		min-height: auto;
		font-size: var(--fs-sm);
		line-height: 1;
		color: var(--text-faint);
	}
	.edit-pending:hover {
		color: var(--accent);
	}
	/* Failed send (CCT-212): the bubble goes red and a Retry control appears. */
	.line.user.failed .bubble {
		background: color-mix(in srgb, var(--danger) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--danger) 50%, transparent);
	}
	.not-delivered {
		color: var(--danger);
		margin-left: auto;
		white-space: nowrap;
	}
	.retry-failed {
		padding: 0 var(--sp-1);
		min-height: auto;
		font-size: var(--fs-sm);
		line-height: 1;
		color: var(--danger);
		font-weight: 600;
	}
	.retry-failed:hover {
		color: color-mix(in srgb, var(--danger) 70%, var(--text));
	}
	/* Working indicator (CCT-208) — animated dots + label proving claude is
	   processing the turn, styled like a muted assistant-side status line. */
	.working {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-1) var(--sp-3);
		color: var(--text-muted);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
	}
	.working-label {
		letter-spacing: 0.02em;
	}
	.working-dots {
		display: inline-flex;
		gap: 3px;
	}
	.working-dots span {
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--role-assistant, var(--accent));
		animation: working-bounce 1.2s var(--ease) infinite;
	}
	.working-dots span:nth-child(2) {
		animation-delay: 0.18s;
	}
	.working-dots span:nth-child(3) {
		animation-delay: 0.36s;
	}
	@keyframes working-bounce {
		0%,
		60%,
		100% {
			opacity: 0.3;
			transform: translateY(0);
		}
		30% {
			opacity: 1;
			transform: translateY(-3px);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.working-dots span {
			animation: none;
			opacity: 0.6;
		}
	}
	.line.tool .bubble,
	.line.result .bubble {
		background: var(--bg-elevated-2);
		border-left: 2px solid color-mix(in srgb, var(--role-tool) 55%, transparent);
	}
	.line.tool.mcp .bubble {
		border-left-color: color-mix(in srgb, var(--role-mcp) 60%, transparent);
	}
	/* Context-reset boundary (/clear or /compact, CCT-158) — a full-width rule
	   with a centered chip in its own blue hue, distinct from the green user,
	   violet system, and amber pending tints. */
	.reset-divider {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		margin: var(--sp-3) 0;
		color: var(--role-boundary);
	}
	.reset-divider::before,
	.reset-divider::after {
		content: '';
		flex: 1;
		height: 1px;
		background: color-mix(in srgb, var(--role-boundary) 40%, transparent);
	}
	.reset-chip {
		padding: 2px var(--sp-3);
		border-radius: var(--r-pill, 999px);
		border: 1px solid color-mix(in srgb, var(--role-boundary) 45%, transparent);
		background: color-mix(in srgb, var(--role-boundary) 12%, var(--bg-elevated));
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		white-space: nowrap;
	}
	/* Compact-summary block (/compact, CCT-159) — its own blue hue, distinct
	   from the green user, violet system, and amber pending bubbles. A filled
	   left-bordered block (not the thin reset divider) so the two boundary
	   kinds read differently. */
	.compact-block {
		margin: var(--sp-3) 0;
		padding: var(--sp-2) var(--sp-3);
		border-left: 3px solid var(--role-boundary);
		border-radius: var(--r-2, 6px);
		background: color-mix(in srgb, var(--role-boundary) 10%, var(--bg-elevated));
	}
	.compact-head {
		color: var(--role-boundary);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		margin-bottom: var(--sp-1);
	}
	.compact-body {
		font-size: var(--fs-sm);
		opacity: 0.9;
	}
	.code {
		white-space: pre-wrap;
		max-height: 22rem;
		overflow: auto;
		font-size: calc(var(--fs-sm) - 0.0625rem);
	}
	.tg.font {
		gap: var(--sp-2);
	}
	.tg.font input[type='range'] {
		width: 5rem;
		accent-color: var(--accent);
	}
	.tg.font span {
		font-weight: var(--fw-bold);
	}
	/* Jump-to-bottom pill (CCT-161 item 7) — anchored to the bottom of the chat
	   display area (inside .conv-wrap), so it never collides with the composer
	   as the textarea grows when typing a long message. */
	.jump-pill {
		position: absolute;
		left: 50%;
		transform: translateX(-50%);
		bottom: var(--sp-3);
		z-index: 3;
		padding: var(--sp-1) var(--sp-3);
		border-radius: var(--r-pill);
		border: 1px solid var(--border-strong);
		background: var(--bg-elevated-2);
		color: var(--text);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		box-shadow: var(--shadow-md);
		cursor: pointer;
	}
	.jump-pill:hover {
		border-color: var(--accent);
		color: var(--accent);
	}
	.composer {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		padding-bottom: calc(var(--sp-3) + var(--safe-bottom));
		border-top: 1px solid var(--border);
		background: var(--bg-elevated);
	}
	/* Highlight the composer while a file drag hovers the conversation pane
	   (CCT-236). */
	.composer.dropping {
		outline: 2px dashed var(--c-blue);
		outline-offset: -2px;
		background: color-mix(in srgb, var(--c-blue) 8%, var(--bg-elevated));
	}
	.composer-row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
		align-items: flex-end;
	}
	.attachments {
		width: 100%;
	}
	/* Composer attach button — uniform control height (CCT-250 item 1). */
	.attach-btn {
		flex: none;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: var(--control-height);
		height: var(--control-height);
		border: 1px solid var(--border-strong);
		border-radius: var(--r-md);
		background: var(--bg);
		cursor: pointer;
		font-size: var(--fs-md);
	}
	.attach-btn:hover {
		border-color: var(--c-blue);
	}
	.file-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		opacity: 0;
		pointer-events: none;
	}
	.attn-banner {
		padding: var(--sp-2) var(--sp-3);
		background: var(--attention-bg);
		border-bottom: 1px solid var(--attention-bar);
		color: var(--warn);
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
	}
	.composer .textarea {
		flex: 1;
		min-height: var(--control-height);
		max-height: 40vh;
		resize: none;
		overflow-y: auto;
	}
	/* Send button matches the attach button + textarea collapsed height. */
	.send {
		flex: none;
		min-height: var(--control-height);
	}
	/* Cold-cache burst (CCT-189): the next send re-writes the whole context to
	   cache, so the normally-green Send button goes blue to flag the cost. */
	.send.cold {
		background: var(--c-blue);
		box-shadow: 0 0 0 1px color-mix(in srgb, var(--c-blue) 40%, transparent);
		color: #fff;
	}
	/* Final-minute warm-window countdown (CCT-261): amber to nudge a send before
	   the cache cools. Loses to .cold (which only applies once already lapsed). */
	.send.warning {
		background: var(--c-amber);
		box-shadow: 0 0 0 1px color-mix(in srgb, var(--c-amber) 40%, transparent);
		color: #fff;
	}
	/* Fixed-width, tabular digits so "59s"→"0s" doesn't jitter the button. */
	.send .countdown {
		display: inline-block;
		min-width: 2.4ch;
		text-align: right;
		font-variant-numeric: tabular-nums;
	}
	.hint {
		font-size: var(--fs-sm);
		text-align: center;
		width: 100%;
	}
	.hint .link {
		color: var(--c-blue);
		font: inherit;
		text-decoration: underline;
		cursor: pointer;
	}
	/* Base prose: grayish (Claude-Code terminal feel, CCT-161 item 5). */
	.bubble {
		color: var(--md-text);
	}
	:global(.bubble strong) {
		color: var(--md-strong);
		font-weight: var(--fw-bold);
	}
	:global(.bubble .md-h) {
		display: inline-block;
		color: var(--md-heading);
		font-weight: var(--fw-bold);
	}
	:global(.bubble .md-quote) {
		display: inline-block;
		border-left: 2px solid var(--border-strong);
		padding-left: var(--sp-2);
		color: var(--text-faint);
	}
	:global(.bubble .md-li) {
		display: inline-block;
	}
	/* Leaked harness pseudo-tags rendered as a muted inline chip. */
	:global(.bubble .md-meta-tag) {
		font-family: var(--font-mono);
		font-size: 0.9em;
		color: var(--text-faint);
		background: var(--bg-elevated-2);
		border-radius: 4px;
		padding: 0 3px;
	}
	:global(.bubble .md-pre) {
		background: var(--bg);
		padding: var(--sp-2);
		border-radius: var(--r-sm);
		overflow-x: auto;
		white-space: pre-wrap;
		color: var(--text);
		margin: var(--sp-1) 0;
	}
	:global(.bubble .md-code) {
		color: var(--md-code);
		font-weight: var(--fw-semibold);
		background: var(--md-code-bg);
		padding: 1px 4px;
		border-radius: 4px;
	}
	/* GFM tables (CCT-233): themed borders + header row, horizontally scrollable
	   so wide tables don't blow out the bubble width. */
	:global(.bubble .md-table) {
		display: block;
		max-width: 100%;
		overflow-x: auto;
		border-collapse: collapse;
		margin: var(--sp-2) 0;
		font-size: var(--fs-sm);
	}
	:global(.bubble .md-table th),
	:global(.bubble .md-table td) {
		border: 1px solid var(--border-strong);
		padding: var(--sp-1) var(--sp-2);
		text-align: left;
		vertical-align: top;
	}
	:global(.bubble .md-table th) {
		background: var(--bg-elevated);
		font-weight: var(--fw-semibold);
		color: var(--text);
	}
	:global(.bubble .md-table tbody tr:nth-child(even)) {
		background: color-mix(in srgb, var(--bg-elevated) 45%, transparent);
	}
	/* Syntax-highlight token colors (CCT-161 item 5) — all themeable. */
	:global(.syn-keyword) {
		color: var(--syn-keyword);
	}
	:global(.syn-string) {
		color: var(--syn-string);
	}
	:global(.syn-number) {
		color: var(--syn-number);
	}
	:global(.syn-comment) {
		color: var(--syn-comment);
		font-style: italic;
	}
	:global(.syn-function) {
		color: var(--syn-function);
	}
</style>
