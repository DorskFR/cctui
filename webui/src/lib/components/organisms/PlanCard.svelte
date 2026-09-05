<script lang="ts">
	import { renderMarkdown } from '$lib/markdown';
	import { Button, Card, Heading, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		plan,
		interactive,
		onsubmit
	}: {
		// The plan markdown the agent presented via ExitPlanMode.
		plan: string;
		interactive: boolean;
		/** Mirrors AskQuestionCard.onsubmit: `picks` is the structured
		 * single-select choice (a lone `[[index]]`) for the digit-answerable
		 * continuations (1-3), or `null` for the free-text "Tell Claude what to
		 * change" refinement — the daemon answers the real PTY form natively from
		 * picks and falls back to dismiss-then-reply for free text. */
		onsubmit: (text: string, picks: number[][] | null) => void;
	} = $props();

	// The continuation options, mirroring the ExitPlanMode PTY prompt order.
	// 1-3 are digit-answerable picks; "refine" opens the free-text field.
	const OPTIONS = [
		{ label: m.plan_opt_auto_accept(), pick: 0 },
		{ label: m.plan_opt_manual_approve(), pick: 1 },
		{ label: m.plan_opt_keep_planning(), pick: 2 }
	];

	// Optimistic local lock (mirrors AskQuestionCard): flip the card to its
	// in-flight state on click rather than waiting for the server round-trip.
	let submitted = $state(false);
	const live = $derived(interactive && !submitted);
	// Release the lock if the parent re-enables the card after a failed answer.
	// svelte-ignore state_referenced_locally
	let wasInteractive = interactive;
	$effect(() => {
		if (interactive && !wasInteractive) submitted = false;
		wasInteractive = interactive;
	});

	// Free-text refinement ("Tell Claude what to change"): revealed when the user
	// chooses to refine rather than accept/keep-planning.
	let refining = $state(false);
	let refineText = $state('');

	function choose(opt: { label: string; pick: number }) {
		if (!live) return;
		submitted = true;
		onsubmit(opt.label, [[opt.pick]]);
	}

	function sendRefine() {
		if (!live || !refineText.trim()) return;
		submitted = true;
		onsubmit(refineText.trim(), null);
	}
</script>

<Card
	tone={live ? 'attention' : 'neutral'}
	surface={live ? 'base' : 'raised'}
	padding="sm"
	gap="var(--sp-3)"
	style="margin:var(--sp-2) 0{live ? '' : ';opacity:0.7'}"
>
	<Heading level={3} size="sm">{m.plan_heading()}</Heading>
	<div class="plan-body">{@html renderMarkdown(plan)}</div>
	{#if live}
		<div class="opts">
			{#each OPTIONS as o (o.pick)}
				<Button variant={o.pick === 2 ? 'default' : 'primary'} onclick={() => choose(o)}>
					{o.label}
				</Button>
			{/each}
			{#if refining}
				<div class="refine">
					<textarea
						class="refine-in"
						placeholder={m.plan_refine_prompt()}
						bind:value={refineText}
					></textarea>
					<Button variant="primary" disabled={!refineText.trim()} onclick={sendRefine}>{m.plan_send()}</Button>
				</div>
			{:else}
				<Button variant="ghost" onclick={() => (refining = true)}>{m.plan_refine_prompt()}</Button>
			{/if}
		</div>
	{:else if submitted && interactive}
		<Text as="div" tone="muted" size="xs" style="font-style:italic">{m.plan_answering()}</Text>
	{:else}
		<Text as="div" tone="muted" size="xs" style="font-style:italic">{m.plan_answered()}</Text>
	{/if}
</Card>

<style>
	.plan-body {
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg);
		padding: var(--sp-3);
		max-height: 480px;
		overflow: auto;
	}
	.opts {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		align-items: flex-start;
	}
	.refine {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		width: 100%;
	}
	.refine-in {
		width: 100%;
		min-height: 64px;
		background: var(--bg-elevated-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		color: var(--text);
		padding: var(--sp-2);
		resize: vertical;
		outline: none;
	}
</style>
