<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { AgentEvent } from '@bindings/AgentEvent';
	import { ws, type PermReq } from '$lib/ws.svelte';
	import { useConversation, useSessionActions } from '$lib/queries';
	import { renderMarkdown, prettyJson } from '$lib/markdown';
	import { clockTime, statusBadgeClass } from '$lib/format';
	import { drafts, composerKey, history as msgHistory, clearSessionStorage, VIEW_OPTS } from '$lib/drafts';
	import { autoresize } from '$lib/autoresize';
	import { toasts } from '$lib/toast.svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { qk } from '$lib/queries';
	import AdapterIcon from './AdapterIcon.svelte';
	import MachineBadge from './MachineBadge.svelte';
	import TokenUsage from './TokenUsage.svelte';
	import PermissionCard from './PermissionCard.svelte';
	import AskQuestionCard from './AskQuestionCard.svelte';

	let { session, onclose }: { session: SessionListItem; onclose: () => void } = $props();

	const id = $derived(session.id);
	const archived = $derived(session.status === 'archived');
	const needsInput = $derived(session.attention === 'needs_input' && !archived);
	const qc = useQueryClient();

	interface ViewOpts {
		showTool: boolean;
		showMcp: boolean;
		showResult: boolean;
		prettyJson: boolean;
		prettyDiff: boolean;
	}
	const defaults: ViewOpts = {
		showTool: true,
		showMcp: false,
		showResult: true,
		prettyJson: true,
		prettyDiff: true
	};
	let view = $state<ViewOpts>(loadView());
	function loadView(): ViewOpts {
		try {
			return { ...defaults, ...JSON.parse(drafts.get(VIEW_OPTS) || '{}') };
		} catch {
			return { ...defaults };
		}
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
	// Timestamps of optimistic replies not yet acknowledged → "sending…" tint.
	let pendingReplies = $state<Set<number>>(new Set());

	// Bumped to force a full re-subscribe + history refetch (e.g. when the tab
	// regains focus after the ws may have gone half-open while backgrounded).
	let resubTick = $state(0);

	// (Re)subscribe + register listeners when the open session changes (or on a
	// forced resubscribe).
	$effect(() => {
		const sid = id;
		void resubTick;
		live = ws.bufferedEvents(sid);
		pendingReplies = new Set();
		notSent = false;
		histIndex = -1;
		draftStash = '';
		ws.subscribe(sid);
		const offStream = ws.onStream(sid, (ev) => {
			// Skip a server-echoed reply that duplicates our optimistic one.
			if (
				ev.type === 'reply' &&
				live.some((e) => e.type === 'reply' && e.content === ev.content)
			) {
				return;
			}
			live = [...live, ev];
			// Any real agent event means our queued replies were received.
			if (ev.type !== 'reply' && pendingReplies.size) pendingReplies = new Set();
		});
		const offPerms = ws.onPerms(sid, (list) => {
			perms = list;
		});
		return () => {
			offStream();
			offPerms();
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

	// History (fetched) + live (ws) events, merged in order.
	const events = $derived([...($history.data ?? []), ...live]);

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
		text?: string;
		tool?: string;
		pending?: boolean;
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
	// (there is no `reply` row on read). Detect that marker so the user's own
	// messages render as user bubbles instead of blending into assistant text.
	const USER_PREFIX = '▷ User:';

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
	function userOrSystem(content: string, ts: number, meta: boolean): Line {
		const role = meta ? 'system' : 'user';
		return { role, ts, html: renderMarkdown(content), text: content };
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
				return { role: 'assistant', ts: Number(e.ts), html: renderMarkdown(e.content), text: e.content };
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
				if (!view.showTool) return null;
				if (isMcp && !view.showMcp) return null;
				return { role: 'tool', ts: Number(e.ts), tool: e.tool, text: formatToolInput(e.tool, e.input) };
			}
			case 'tool_result':
				if (!view.showResult) return null;
				return { role: 'result', ts: Number(e.ts), tool: e.tool, text: e.output_summary };
			case 'context_reset':
				// /clear: the session id rotated under the same worker (CCT-158).
				// Render as a distinct full-width boundary.
				return { role: 'reset', ts: Number(e.ts), text: 'context reset · /clear or /compact' };
			case 'compact_summary':
				// /compact appends a summary in place (no session-id rotation),
				// so it arrives with its text (CCT-159). Render as a distinct
				// "context compacted" block rather than a user bubble.
				if (!e.content.trim()) return null;
				return { role: 'compact', ts: Number(e.ts), html: renderMarkdown(e.content), text: e.content };
			default:
				return null; // heartbeat, turn_end
		}
	}

	function formatToolInput(tool: string, input: unknown): string {
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
			return `${obj.file_path ?? ''}\n${minus}\n${plus}`.trim();
		}
		return view.prettyJson ? prettyJson(input) : JSON.stringify(input);
	}

	// Build lines with consecutive-duplicate dedup.
	const pendingTs = $derived(pendingReplies);
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
			if (ln.role === 'user' && pendingTs.has(ln.ts)) ln.pending = true;
			out.push(ln);
		}
		return out;
	});

	const actions = useSessionActions();

	// Composer
	let input = $state(drafts.get(composerKey(session.id)));
	$effect(() => {
		drafts.set(composerKey(session.id), input);
	});

	// Set when a send was dropped because the socket wasn't OPEN — shown inline
	// near the composer so the user knows their (still-present) text wasn't sent.
	let notSent = $state(false);

	// ── Sent-message history recall (ArrowUp/ArrowDown) ─────────────────────
	// histIndex: -1 = editing the live draft; 0..n-1 = browsing history
	// (newest-first as you press Up). draftStash holds the in-progress text so
	// returning past the newest entry restores it.
	let histIndex = $state(-1);
	let draftStash = '';
	function resetHistoryNav() {
		histIndex = -1;
	}

	function send() {
		const text = input.trim();
		if (!text || archived) return;
		const ok = ws.sendMessage(id, text);
		if (!ok) {
			// Socket wasn't OPEN: the frame was dropped. Keep the draft, show no
			// phantom echo, and surface a "not sent — reconnecting" notice. Nudge
			// a reconnect so the user can retry once it's back.
			notSent = true;
			ws.connect();
			return;
		}
		notSent = false;
		// Optimistic echo into local state (+ pending tint until the agent replies).
		const ts = Date.now();
		live = [...live, { type: 'reply', content: text, ts }];
		pendingReplies = new Set([...pendingReplies, ts]);
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

	// Answer an AskUserQuestion (CCT-146). cctui has no structured tool-result
	// channel, so the selection is sent as a reply message — the claude control
	// socket's `reply` op advances the turn, which is how the agent continues.
	function answerQuestion(text: string) {
		if (archived) return;
		const ok = ws.sendMessage(id, text);
		if (!ok) {
			notSent = true;
			ws.connect();
			return;
		}
		notSent = false;
		const ts = Date.now();
		live = [...live, { type: 'reply', content: text, ts }];
		pendingReplies = new Set([...pendingReplies, ts]);
		qc.invalidateQueries({ queryKey: ['sessions'] });
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
			// history) — it's gone once archived (CCT-162).
			clearSessionStorage(session.id);
			toasts.ok('Archived');
			onclose();
		} catch (e) {
			toasts.err((e as Error).message);
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

	// auto-scroll to bottom on new lines
	let scroller = $state<HTMLElement>();
	$effect(() => {
		void lines.length;
		if (scroller) scroller.scrollTop = scroller.scrollHeight;
	});
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

<div class="drawer">
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
			<MachineBadge name={session.machine_name} id={session.machine_id} mono />
			<button
				class="chip mono cwd truncate"
				title="Click to copy — {session.working_dir}"
				onclick={() => copyLine(session.working_dir)}
			>📁 {session.working_dir} ⧉</button>
			<TokenUsage usage={session.token_usage} />
		</div>
	</div>

	<div class="toggles row row-wrap">
		<label class="tg"><input type="checkbox" bind:checked={view.showTool} /> Tools</label>
		<label class="tg"><input type="checkbox" bind:checked={view.showMcp} /> MCP</label>
		<label class="tg"><input type="checkbox" bind:checked={view.showResult} /> Results</label>
		<label class="tg"><input type="checkbox" bind:checked={view.prettyJson} /> JSON</label>
		<label class="tg"><input type="checkbox" bind:checked={view.prettyDiff} /> Diff</label>
		<label class="tg auto" title="Auto-approve permission requests for this session">
			<input type="checkbox" checked={session.auto_approve} onchange={toggleAutoApprove} /> Auto-approve
		</label>
	</div>

	{#if needsInput}
		<div class="attn-banner">✋ Waiting for your input</div>
	{/if}

	<div class="conv" bind:this={scroller}>
		{#if $history.isLoading}
			<div class="empty"><span class="spin"></span></div>
		{:else if lines.length === 0 && perms.length === 0}
			<div class="empty">No events yet.</div>
		{/if}

		{#each lines as ln, i (ln.ts + (ln.text ?? ln.html ?? '').slice(0, 24) + ln.role)}
			{#if ln.ask}
				<AskQuestionCard
					questions={ln.ask}
					interactive={i === lines.length - 1 && !archived}
					onsubmit={answerQuestion}
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
			<div class="line {ln.role}" class:pending={ln.pending}>
				<div class="lmeta row">
					<span class="who">{ln.role === 'tool' ? (ln.tool ?? 'tool') : ln.role === 'result' ? `↳ ${ln.tool ?? ''}` : ln.role}</span>
					<span class="faint sm">{clockTime(ln.ts)}</span>
					{#if ln.pending}<span class="faint sm sending">sending…</span>{/if}
					<button class="btn btn-ghost copy" aria-label="Copy" onclick={() => copyLine(ln.text ?? '')}>⧉</button>
				</div>
				{#if ln.html}
					<div class="bubble">{@html ln.html}</div>
				{:else}
					<pre class="bubble mono code">{ln.text}</pre>
				{/if}
			</div>
			{/if}
		{/each}

		{#each perms as p (p.request_id)}
			<PermissionCard req={p} onrespond={(rid, allow) => ws.respondPermission(id, rid, allow)} />
		{/each}
	</div>

	<div class="composer">
		{#if archived}
			<div class="hint muted">Session archived — unarchive to send messages.</div>
		{:else}
			{#if notSent}
				<div class="notsent" role="status">⚠ Not sent — reconnecting. Your message is kept; press Send to retry.</div>
			{/if}
			<textarea
				class="textarea grow"
				rows="1"
				placeholder="Message… (Enter to send, Shift+Enter for newline)"
				bind:value={input}
				bind:this={textarea}
				onkeydown={onKey}
				oninput={() => {
					resetHistoryNav();
					if (notSent) notSent = false;
				}}
				use:autoresize={input}
			></textarea>
			<button class="btn btn-primary send" disabled={!input.trim()} onclick={send}>Send</button>
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
			width: min(900px, 100vw);
			border-left: 1px solid var(--border);
			box-shadow: -4px 0 24px rgba(0, 0, 0, 0.4);
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
	.tg.auto {
		margin-left: auto;
		font-weight: 600;
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
	.toggles {
		gap: var(--sp-3);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
		overflow-x: auto;
		font-size: var(--fs-xs);
	}
	.tg {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		white-space: nowrap;
		color: var(--text-muted);
	}
	.conv {
		flex: 1;
		overflow-y: auto;
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
		font-size: var(--fs-sm);
	}
	.line.user .bubble {
		background: color-mix(in srgb, var(--accent) 14%, var(--bg-elevated));
		border-color: var(--accent-dim);
	}
	/* System/agent-directed messages (harness wake-ups, task notifications,
	   injected reminders) — violet, distinct from the green user bubbles so
	   they don't read as something the human typed. */
	.line.system .bubble {
		background: color-mix(in srgb, var(--c-violet) 12%, var(--bg-elevated));
		border-color: color-mix(in srgb, var(--c-violet) 40%, transparent);
	}
	.line.system .who {
		color: var(--c-violet);
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
	.line.tool .bubble,
	.line.result .bubble {
		background: var(--bg-elevated-2);
	}
	/* Context-reset boundary (/clear or /compact, CCT-158) — a full-width rule
	   with a centered chip in its own blue hue, distinct from the green user,
	   violet system, and amber pending tints. */
	.reset-divider {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		margin: var(--sp-3) 0;
		color: var(--info);
	}
	.reset-divider::before,
	.reset-divider::after {
		content: '';
		flex: 1;
		height: 1px;
		background: color-mix(in srgb, var(--info) 40%, transparent);
	}
	.reset-chip {
		padding: 2px var(--sp-3);
		border-radius: var(--r-pill, 999px);
		border: 1px solid color-mix(in srgb, var(--info) 45%, transparent);
		background: color-mix(in srgb, var(--info) 12%, var(--bg-elevated));
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
		border-left: 3px solid var(--info);
		border-radius: var(--r-2, 6px);
		background: color-mix(in srgb, var(--info) 10%, var(--bg-elevated));
	}
	.compact-head {
		color: var(--info);
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
		font-size: var(--fs-xs);
	}
	.composer {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
		align-items: flex-end;
		padding: var(--sp-3);
		padding-bottom: calc(var(--sp-3) + var(--safe-bottom));
		border-top: 1px solid var(--border);
		background: var(--bg-elevated);
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
		min-height: 2.75rem;
		max-height: 40vh;
		resize: none;
		overflow-y: auto;
	}
	.send {
		flex: none;
	}
	.hint {
		font-size: var(--fs-sm);
		text-align: center;
		width: 100%;
	}
	/* Inline notice when a send was dropped (socket not OPEN). Full-width so it
	   sits on its own line above the textarea (CCT-162). */
	.notsent {
		width: 100%;
		font-size: var(--fs-xs);
		color: var(--warn);
		background: color-mix(in srgb, var(--warn) 12%, var(--bg-elevated));
		border: 1px solid color-mix(in srgb, var(--warn) 35%, transparent);
		border-radius: var(--r-sm);
		padding: var(--sp-1) var(--sp-2);
	}
	:global(.bubble .md-pre) {
		background: var(--bg);
		padding: var(--sp-2);
		border-radius: var(--r-sm);
		overflow-x: auto;
		white-space: pre-wrap;
	}
	:global(.bubble .md-code) {
		background: var(--bg);
		padding: 1px 4px;
		border-radius: 4px;
	}
</style>
