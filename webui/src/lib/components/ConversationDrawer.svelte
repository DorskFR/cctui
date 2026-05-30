<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { AgentEvent } from '@bindings/AgentEvent';
	import { ws, type PermReq } from '$lib/ws.svelte';
	import { useConversation, useSessionActions } from '$lib/queries';
	import { renderMarkdown, prettyJson } from '$lib/markdown';
	import { clockTime, statusBadgeClass } from '$lib/format';
	import { drafts, composerKey, VIEW_OPTS } from '$lib/drafts';
	import { autoresize } from '$lib/autoresize';
	import { toasts } from '$lib/toast.svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import BrandLogo from './BrandLogo.svelte';
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

	// (Re)subscribe + register listeners when the open session changes.
	$effect(() => {
		const sid = id;
		live = ws.bufferedEvents(sid);
		pendingReplies = new Set();
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

	// History (fetched) + live (ws) events, merged in order.
	const events = $derived([...($history.data ?? []), ...live]);

	interface AskQuestion {
		header?: string;
		question: string;
		multiSelect?: boolean;
		options: { label: string; description?: string; preview?: string }[];
	}
	interface Line {
		role: 'assistant' | 'user' | 'tool' | 'result';
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

	function toLine(e: AgentEvent): Line | null {
		switch (e.type) {
			case 'text': {
				// Streaming emits an empty text event before the populated one —
				// skip empties so they don't render as blank assistant blocks.
				if (!e.content.trim()) return null;
				if (e.content.startsWith(USER_PREFIX)) {
					const content = e.content.slice(USER_PREFIX.length).trimStart();
					return { role: 'user', ts: Number(e.ts), html: renderMarkdown(content), text: content };
				}
				return { role: 'assistant', ts: Number(e.ts), html: renderMarkdown(e.content), text: e.content };
			}
			case 'reply':
				if (!e.content.trim()) return null;
				return { role: 'user', ts: Number(e.ts), html: renderMarkdown(e.content), text: e.content };
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
			const key = `${ln.role}|${ln.tool ?? ''}|${ln.text ?? ln.html ?? ''}`;
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

	function send() {
		const text = input.trim();
		if (!text || archived) return;
		ws.sendMessage(id, text);
		// Optimistic echo into local state (+ pending tint until the agent replies).
		const ts = Date.now();
		live = [...live, { type: 'reply', content: text, ts }];
		pendingReplies = new Set([...pendingReplies, ts]);
		input = '';
		drafts.clear(composerKey(session.id));
		// Reflect the new turn in the list (last-message / ordering) without
		// waiting for the next poll.
		qc.invalidateQueries({ queryKey: ['sessions'] });
	}
	function onKey(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			send();
		}
	}

	// Answer an AskUserQuestion (CCT-146). cctui has no structured tool-result
	// channel, so the selection is sent as a reply message — the claude control
	// socket's `reply` op advances the turn, which is how the agent continues.
	function answerQuestion(text: string) {
		if (archived) return;
		ws.sendMessage(id, text);
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

<div class="drawer">
	<div class="dhead">
		<div class="hrow">
			<button class="tapbtn back" aria-label="Back" onclick={onclose}>‹</button>
			<span class="hlogo" class:codex={String(session.adapter_id ?? '').startsWith('codex')}>
				<BrandLogo adapter={session.adapter_id} size={20} />
			</span>
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
			<span class="chip mono">{session.machine_name ?? session.machine_id.slice(0, 8)}</span>
			<span class="chip mono cwd truncate" title={session.working_dir}>📁 {session.working_dir}</span>
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
			<textarea
				class="textarea grow"
				rows="1"
				placeholder="Message… (Enter to send, Shift+Enter for newline)"
				bind:value={input}
				onkeydown={onKey}
				use:autoresize={input}
			></textarea>
			<button class="btn btn-primary send" disabled={!input.trim()} onclick={send}>Send</button>
		{/if}
	</div>
</div>

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
	.hlogo {
		display: inline-flex;
		align-items: center;
		color: var(--c-amber);
		flex: none;
	}
	.hlogo.codex {
		color: var(--c-blue);
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
	.chip.cwd {
		flex: 1;
		min-width: 6rem;
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
	.code {
		white-space: pre-wrap;
		max-height: 22rem;
		overflow: auto;
		font-size: var(--fs-xs);
	}
	.composer {
		display: flex;
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
