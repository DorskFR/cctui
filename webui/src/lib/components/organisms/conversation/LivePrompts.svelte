<script lang="ts">
	// Prompts awaiting the user at the tail of the transcript: the live
	// AskUserQuestion, the live ExitPlanMode plan, the current task list and any
	// pending permission requests.
	import PermissionCard from '$lib/components/organisms/PermissionCard.svelte';
	import AskQuestionCard from '$lib/components/organisms/AskQuestionCard.svelte';
	import PlanCard from '$lib/components/organisms/PlanCard.svelte';
	import TodoCard from '$lib/components/organisms/TodoCard.svelte';
	import type { ConversationStream } from './stream.svelte';
	import type { Line } from './types';

	let {
		stream,
		lines,
		archived,
		askPreambleHtml,
		planPreambleHtml,
		onrespondperm
	}: {
		stream: ConversationStream;
		lines: Line[];
		archived: boolean;
		askPreambleHtml: string | null;
		planPreambleHtml: string | null;
		onrespondperm: (requestId: string, allow: boolean) => void;
	} = $props();

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
	     never re-seeded — which left the new answer un-submittable / stuck. -->
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

{#if stream.todos}
	<TodoCard todos={stream.todos} />
{/if}

{#each stream.perms as p (p.request_id)}
	<PermissionCard req={p} onrespond={(rid, allow) => onrespondperm(rid, allow)} />
{/each}

<style>
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
</style>
