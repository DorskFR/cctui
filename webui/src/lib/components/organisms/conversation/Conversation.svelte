<script lang="ts">
	import AskQuestionCard from '$lib/components/organisms/AskQuestionCard.svelte';
	import PlanCard from '$lib/components/organisms/PlanCard.svelte';
	import TodoCard from '$lib/components/organisms/TodoCard.svelte';
	import { Button, EmptyState, Text } from '@dorsk/tsumikit';
	import BoundaryLine from './BoundaryLine.svelte';
	import ConversationLine from './ConversationLine.svelte';
	import LivePrompts from './LivePrompts.svelte';
	import TurnSummaryFooter from './TurnSummaryFooter.svelte';
	import { latestTodoLineKey } from './format';
	import { copyLineMarkdown, saveLineImage } from './lineActions';
	import type { ScrollController } from './scroll.svelte';
	import type { ConversationStream } from './stream.svelte';
	import type { RenderWindow } from './jump';
	import type { Line } from './types';
	import type { PluginActionButton } from '$lib/plugins/types';
	import { m } from '$lib/paraglide/messages';
	import { untrack } from 'svelte';

	let {
		stream,
		scroll,
		sessionId,
		lines,
		isLoading,
		canFetchOlder = false,
		fetchingOlder = false,
		onfetcholder,
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
		ontoggleselect,
		pinnedSeqs = new Set<number>(),
		onpin,
		jumper = $bindable(),
		focusTs = null,
		onbookmark,
		isBookmarked,
		pluginActionsFor,
		onpluginaction
	}: {
		/** Live-stream controller. Passed whole rather than as a dozen
		 * pass-through props; its `$state` fields stay reactive when read through it. */
		stream: ConversationStream;
		scroll: ScrollController;
		sessionId: string;
		lines: Line[];
		isLoading: boolean;
		// Server-side paging: more history exists beyond the fetched window.
		canFetchOlder?: boolean;
		fetchingOlder?: boolean;
		onfetcholder?: () => Promise<void>;
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
		pinnedSeqs?: Set<number>;
		onpin?: (ln: Line) => void;
		/** Bound out: the render-window half of `ensureSeqVisible` lives here,
		 *  since `renderLimit` is component state. */
		jumper?: RenderWindow;
		/** `ts` of the searched-for message, when opened from a search hit; gets
		 *  the persistent focus ring and is scrolled to on open. */
		focusTs?: number | null;
		onbookmark?: (ln: Line) => void;
		isBookmarked?: (ln: Line) => boolean;
		/** Buttons runtime plugins contribute to an assistant line. */
		pluginActionsFor?: (ln: Line) => PluginActionButton[];
		onpluginaction?: (a: PluginActionButton) => void;
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
	jumper = {
		isRendered: (seq: number) => {
			const i = lines.findIndex((l) => l.seq === seq);
			return i >= 0 && i >= lines.length - renderLimit;
		},
		grow: () => scroll.holdForPrepend(() => (renderLimit += RENDER_CHUNK))
	};
	const visibleLines = $derived(hiddenOlder > 0 ? lines.slice(hiddenOlder) : lines);
	const latestTodoKey = $derived(latestTodoLineKey(lines));
	export async function loadOlder() {
		if (hiddenOlder === 0 && canFetchOlder && onfetcholder) await onfetcholder();
		scroll.holdForPrepend(() => (renderLimit += RENDER_CHUNK));
	}

	// ── Search focus ────────────────────────────────────────────────────────
	// Matched by `ts` rather than causal seq: `Line` carries no seq today. The
	// drawer resolves focusSeq → focusTs from the raw events.
	const focusIdx = $derived(focusTs == null ? -1 : lines.findIndex((l) => l.ts === focusTs));
	// The focused line is usually far above the tail render window; widen the
	// window so it mounts at all.
	$effect(() => {
		if (focusIdx < 0) return;
		const needed = lines.length - focusIdx + RENDER_CHUNK;
		if (needed > renderLimit) renderLimit = needed;
	});

	// Dropped on the first real scroll gesture, so the ring marks the hit
	// without following the user around the transcript.
	let ringDismissedAt = $state(0);
	$effect(() => {
		void focusTs;
		ringDismissedAt = untrack(() => scroll.gestures);
	});
	const showFocusRing = $derived(focusIdx >= 0 && scroll.gestures === ringDismissedAt);

	let focusEl = $state<HTMLElement | undefined>(undefined);
	// Only handles the already-rendered-window case; the drawer's
	// `ensureSeqVisible(seq)` pages older history until the seq is in `lines`.
	function scrollFocusLineIntoViewLocal() {
		const el = focusEl;
		if (!el) return;
		el.scrollIntoView({ block: 'center', behavior: 'auto' });
	}
	let scrolledToTs = $state<number | null>(null);
	$effect(() => {
		if (focusTs == null) {
			scrolledToTs = null;
			return;
		}
		if (!focusEl || scrolledToTs === focusTs) return;
		scrolledToTs = focusTs;
		requestAnimationFrame(scrollFocusLineIntoViewLocal);
	});
</script>

{#snippet convLine(ln: Line)}
	<ConversationLine
		{ln}
		{archived}
		{sessionId}
		pinned={ln.seq !== undefined && pinnedSeqs.has(ln.seq)}
		{onpin}
		onretry={(ts) => stream.retryFailed(ts)}
		{onedit}
		onsaveimage={saveLineImage}
		oncopymarkdown={copyLineMarkdown}
		{forkable}
		{selectMode}
		selectedForFork={ln.messageId ? selected.has(ln.messageId) : false}
		{onforkfrom}
		{onforkafter}
		{ontoggleselect}
		{onbookmark}
		bookmarked={isBookmarked?.(ln) ?? false}
		pluginActions={pluginActionsFor?.(ln) ?? []}
		{onpluginaction}
	/>
{/snippet}

<div class="conv-wrap">
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="conv"
		bind:this={scroll.scroller}
		onscroll={scroll.onScroll}
		onwheel={scroll.markScrollGesture}
		ontouchmove={scroll.markScrollGesture}
		onpointerdown={scroll.markUserScroll}
		onkeydown={scroll.markScrollGesture}
	>
		{#if isLoading}
			<EmptyState loading />
		{:else if lines.length === 0 && stream.perms.length === 0 && !stream.ask && !stream.plan}
			<EmptyState size="inline" title={m.conversation_no_events()} />
		{/if}

		{#if hiddenOlder > 0 || canFetchOlder}
			<!-- Lazy render: older lines are mounted on demand so a
			     long transcript opens fast. -->
			<div class="older-row">
				<Button pill size="sm" loading={fetchingOlder} onclick={loadOlder}>
					{m.conversation_load_older({
						count: hiddenOlder > 0 ? Math.min(RENDER_CHUNK, hiddenOlder) : RENDER_CHUNK
					})}
					{#if hiddenOlder > 0}
						<Text tone="faint">{m.conversation_hidden_count({ count: hiddenOlder })}</Text>
					{/if}
				</Button>
			</div>
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
			{:else if ln.todos && (stream.todos || ln.key !== latestTodoKey)}
				<!-- Superseded: only the newest task list renders, so a 20-update
				     turn produces one card and not twenty. -->
			{:else if ln.todos}
				<TodoCard todos={ln.todos} />
			{:else if ln.role === 'reset' || ln.role === 'compact'}
				<BoundaryLine {ln} />
			{:else if ln.role === 'summary' && ln.summary}
				<!-- No assistant bubble to hang this turn summary on; it still shows,
				     as a bare footer. -->
				<TurnSummaryFooter summary={ln.summary} />
			{:else if hiddenOlder + i === focusIdx}
				<!-- The searched-for message: wrapped rather than styled in place so
				     ConversationLine stays untouched. -->
				<div class="focus-wrap" class:line-focus={showFocusRing} bind:this={focusEl}>
					{@render convLine(ln)}
				</div>
			{:else}
				{@render convLine(ln)}
			{/if}
		{/each}

		<LivePrompts
			{stream}
			{lines}
			{archived}
			{askPreambleHtml}
			{planPreambleHtml}
			{onrespondperm}
		/>
	</div>

	{#if !scroll.stuck}
		<div class="jump-anchor">
			<Button pill size="sm" onclick={scroll.jumpToBottom} aria-label={m.conversation_jump_to_bottom()}>
				{m.conversation_jump_to_latest()}
			</Button>
		</div>
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
	/* The searched-for message, opened from a search hit. A ring on a wrapper
	   (ConversationLine is owned elsewhere) so the *message* is findable even
	   when the term also matches a dozen other lines. */
	.focus-wrap {
		display: flex;
		flex-direction: column;
		max-width: 100%;
		border-radius: var(--r-md);
		transition: box-shadow 160ms var(--ease), background 160ms var(--ease);
	}
	.focus-wrap.line-focus {
		box-shadow: 0 0 0 2px var(--accent);
		background: color-mix(in srgb, var(--accent) 10%, transparent);
	}
	@media (prefers-reduced-motion: reduce) {
		.focus-wrap {
			transition: none;
		}
	}
	.older-row {
		display: flex;
		justify-content: center;
	}
	/* Anchored to the bottom of the chat display area (inside .conv-wrap), so the
	   pill never collides with the composer as the textarea grows. */
	.jump-anchor {
		position: absolute;
		left: 50%;
		transform: translateX(-50%);
		bottom: var(--sp-3);
		z-index: 3;
		border-radius: var(--r-pill);
		box-shadow: var(--shadow-md);
	}
</style>
