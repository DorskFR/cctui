<script lang="ts">
	import PermissionCard from '$lib/components/organisms/PermissionCard.svelte';
	import AskQuestionCard from '$lib/components/organisms/AskQuestionCard.svelte';
	import PlanCard from '$lib/components/organisms/PlanCard.svelte';
	import { Button, Text } from '@dorsk/tsumikit';
	import ConversationLine from './ConversationLine.svelte';
	import { copyLineMarkdown, saveLineImage } from './lineActions';
	import type { ScrollController } from './scroll.svelte';
	import type { ConversationStream } from './stream.svelte';
	import type { Line } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		stream,
		scroll,
		sessionId,
		lines,
		isLoading,
		archived,
		askPreambleHtml,
		planPreambleHtml,
		onedit,
		onrespondperm,
		forkable = false,
		selectMode = false,
		selected = new Set<string>(),
		onforkfrom,
		onforkafter,
		ontoggleselect
	}: {
		/** Live-stream controller. Passed whole rather than as a dozen
		 * pass-through props; its `$state` fields stay reactive when read through it. */
		stream: ConversationStream;
		scroll: ScrollController;
		sessionId: string;
		lines: Line[];
		isLoading: boolean;
		archived: boolean;
		askPreambleHtml: string | null;
		planPreambleHtml: string | null;
		onedit: (text: string, ts: number) => void;
		onrespondperm: (requestId: string, allow: boolean) => void;
		// Subset-fork affordances; off for codex/archived sessions.
		forkable?: boolean;
		selectMode?: boolean;
		selected?: Set<string>;
		onforkfrom?: (messageId: string) => void;
		onforkafter?: (messageId: string) => void;
		ontoggleselect?: (messageId: string) => void;
	} = $props();

	// ── Lazy render of large transcripts ───────────────────
	// Mounting an entire long conversation (hundreds of tool calls + results, each
	// running the markdown/highlight pipeline) blocks the open for seconds. Render
	// only the most recent `renderLimit` lines initially and expose a "load older"
	// control that reveals more upward, in chunks. New live events always fall
	// inside the tail window, so auto-scroll-to-bottom is unaffected.
	const RENDER_CHUNK = 60;
	let renderLimit = $state(RENDER_CHUNK);
	// Reset the window when the open session changes.
	$effect(() => {
		void sessionId;
		renderLimit = RENDER_CHUNK;
	});
	const hiddenOlder = $derived(Math.max(0, lines.length - renderLimit));
	const visibleLines = $derived(hiddenOlder > 0 ? lines.slice(hiddenOlder) : lines);
	function loadOlder() {
		scroll.holdForPrepend(() => (renderLimit += RENDER_CHUNK));
	}

	// Suppress the live preamble block when the same assistant prose has already
	// streamed into the transcript.
	const preambleInLines = $derived.by(() => {
		const pre = stream.ask?.preamble?.trim();
		return !!pre && lines.some((l) => l.role === 'assistant' && (l.text ?? '').trim() === pre);
	});
	// Same suppression for the live plan's preamble.
	const planPreambleInLines = $derived.by(() => {
		const pre = stream.plan?.preamble?.trim();
		return !!pre && lines.some((l) => l.role === 'assistant' && (l.text ?? '').trim() === pre);
	});
</script>

<div class="conv-wrap">
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="conv"
		bind:this={scroll.scroller}
		onscroll={scroll.onScroll}
		onwheel={scroll.markUserScroll}
		ontouchmove={scroll.markUserScroll}
		onpointerdown={scroll.markUserScroll}
		onkeydown={scroll.markUserScroll}
	>
		{#if isLoading}
			<div class="empty"><span class="spin"></span></div>
		{:else if lines.length === 0 && stream.perms.length === 0 && !stream.ask && !stream.plan}
			<div class="empty"><Text>{m.conversation_no_events()}</Text></div>
		{/if}

		{#if hiddenOlder > 0}
			<!-- Lazy render: older lines are mounted on demand so a
			     long transcript opens fast. -->
			<Button class="load-older" onclick={loadOlder}>
				{m.conversation_load_older({ count: Math.min(RENDER_CHUNK, hiddenOlder) })}
				<Text tone="faint">{m.conversation_hidden_count({ count: hiddenOlder })}</Text>
			</Button>
		{/if}
		{#each visibleLines as ln, i (ln.key)}
			{#if ln.ask && stream.isDupeOfLiveAsk(ln.ask)}
				<!-- Suppressed: same question is rendered live below. -->
			{:else if ln.ask}
				<AskQuestionCard
					questions={ln.ask}
					interactive={i === visibleLines.length - 1 && !archived && !stream.answering && !stream.ask}
					onsubmit={(t, p) => stream.answerQuestion(t, p, ln.ask)}
				/>
			{:else if ln.plan && stream.plan}
				<!-- Suppressed: a live plan prompt is rendered below. -->
			{:else if ln.plan}
				<PlanCard
					plan={ln.plan}
					interactive={i === visibleLines.length - 1 && !archived && !stream.answering && !stream.plan}
					onsubmit={(t, p) => stream.answerPlan(t, p)}
				/>
			{:else if ln.role === 'reset'}
				<div class="reset-divider" role="separator">
					<span class="reset-chip">⟳ {ln.text}</span>
				</div>
			{:else if ln.role === 'compact'}
				<div class="compact-block">
					<div class="compact-head">{m.conversation_context_compacted()}</div>
					{#if ln.html}<div class="compact-body">{@html ln.html}</div>{/if}
				</div>
			{:else}
				<ConversationLine
					{ln}
					{archived}
					onretry={(ts) => stream.retryFailed(ts)}
					onedit={onedit}
					onsaveimage={saveLineImage}
					oncopymarkdown={copyLineMarkdown}
					{forkable}
					{selectMode}
					selectedForFork={ln.messageId ? selected.has(ln.messageId) : false}
					{onforkfrom}
					{onforkafter}
					{ontoggleselect}
				/>
			{/if}
		{/each}

		{#if stream.ask}
			<!-- Live AskUserQuestion: the daemon's hook forwards the
			     structured options, so render the interactive option-card form live.
			     Older deliveries (no structured payload) fall back to the question
			     text with a free-text answer. Answering sends a reply. -->
			{#if askPreambleHtml && !preambleInLines}
				<!-- The assistant prose preceding the question: the reasoning
				     the choice depends on, so the user isn't blind. -->
				<div class="line assistant ask-preamble">
					<div class="bubble">{@html askPreambleHtml}</div>
				</div>
			{/if}
			<!-- Re-key on the question text so a SUCCESSIVE ask gets a fresh card
			     instance instead of reusing one whose per-question selection state
			     (chosen/other/focused) was seeded from the PREVIOUS ask's prop and
			     never re-seeded — which left the new answer un-submittable / stuck
			    . -->
			{#key stream.ask.question}
				<AskQuestionCard
					questions={stream.liveAskQuestions ?? [{ question: stream.ask.question, options: [] }]}
					interactive={!archived && !stream.answering}
					onsubmit={(t, p) => stream.answerQuestion(t, p, stream.liveAskQuestions)}
				/>
			{/key}
		{/if}

		{#if stream.plan}
			<!-- Live ExitPlanMode plan-approval prompt: the daemon's hook
			     forwards the plan markdown the instant the prompt renders, so render
			     the interactive Plan card live. Answering sends a reply (digit pick
			     1-3 natively, or free-text refine). -->
			{#if planPreambleHtml && !planPreambleInLines}
				<div class="line assistant ask-preamble">
					<div class="bubble">{@html planPreambleHtml}</div>
				</div>
			{/if}
			{#key stream.plan.plan}
				<PlanCard
					plan={stream.plan.plan}
					interactive={!archived && !stream.answering}
					onsubmit={(t, p) => stream.answerPlan(t, p)}
				/>
			{/key}
		{/if}

		{#each stream.perms as p (p.request_id)}
			<PermissionCard req={p} onrespond={(rid, allow) => onrespondperm(rid, allow)} />
		{/each}

		{#if stream.working && !archived && !stream.ask && !stream.plan && stream.perms.length === 0}
			<!-- Activity indicator: proves the request is being processed,
			     the equivalent of the TUI's "Running…" spinner. -->
			<div class="working" role="status" aria-live="polite">
				<span class="working-dots" aria-hidden="true"><span></span><span></span><span></span></span>
				<span class="working-label">{m.conversation_working()}</span>
			</div>
		{/if}
	</div>

	{#if !scroll.stuck}
		<Button class="jump-pill" onclick={scroll.jumpToBottom} aria-label={m.conversation_jump_to_bottom()}>
			{m.conversation_jump_to_latest()}
		</Button>
	{/if}
</div>

<style>
	/* Positioning context for the jump-pill so it anchors to the bottom of the
	   chat display area, never overlapping the (growable) composer. */
	.conv-wrap {
		position: relative;
		flex: 1;
		display: flex;
		flex-direction: column;
		min-height: 0;
		/* Keep vertical scroll native; we handle horizontal swipes. */
		touch-action: pan-y;
	}
	.conv {
		flex: 1;
		overflow-y: auto;
		/* Keep the chat's scroll inside the pane: without this, hitting
		   the top/bottom of a long log chains the swipe to the page behind. */
		overscroll-behavior: contain;
		-webkit-overflow-scrolling: touch;
		padding: var(--sp-3);
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	/* The ask-preamble reuses the `.line.assistant` bubble look; the full
	   per-message line styling lives in ConversationLine.svelte and the bubble
	   base/markdown in bubble.css. */
	.line {
		display: flex;
		flex-direction: column;
		gap: 2px;
		max-width: 100%;
	}
	.line.assistant .bubble {
		border-left: 2px solid color-mix(in srgb, var(--role-assistant) 55%, transparent);
	}
	/* Working indicator — animated dots + label proving claude is
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
	/* Context-reset boundary (/clear or /compact) — a full-width rule with
	   a centered chip in its own blue hue. */
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
	/* Compact-summary block (/compact) — its own blue hue, a filled
	   left-bordered block (not the thin reset divider) so the two boundary kinds
	   read differently. */
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
	/* Lazy-render "load older" control. */
	.conv :global(.load-older) {
		align-self: center;
		padding: var(--sp-1) var(--sp-3);
		min-height: auto;
		border-radius: var(--r-pill);
		border: 1px solid var(--border-strong);
		background: var(--bg-elevated-2);
		color: var(--text-muted);
		font-size: var(--fs-xs);
		font-weight: var(--fw-medium);
		cursor: pointer;
	}
	.conv :global(.load-older:hover) {
		border-color: var(--accent);
		color: var(--accent);
	}
	/* Jump-to-bottom pill — anchored to the bottom of the chat
	   display area (inside .conv-wrap), so it never collides with the composer as
	   the textarea grows when typing a long message. */
	.conv-wrap :global(.jump-pill) {
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
	.conv-wrap :global(.jump-pill:hover) {
		border-color: var(--accent);
		color: var(--accent);
	}
</style>
