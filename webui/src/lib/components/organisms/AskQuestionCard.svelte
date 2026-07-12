<script lang="ts">
	import { renderMarkdown } from '$lib/markdown';
	import { Badge, Button, Heading, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	interface Opt {
		label: string;
		description?: string;
		preview?: string;
	}
	interface Question {
		header?: string;
		question: string;
		multiSelect?: boolean;
		options: Opt[];
	}

	let {
		questions,
		interactive,
		onsubmit
	}: {
		questions: Question[];
		interactive: boolean;
		/** `picks` is the structured per-question option selection (0-based
		 * indices) when every answer is a pure option pick, or `null` when any
		 * question used the free-text "Other…" field — the daemon answers the
		 * real form natively from picks (CCT-226) and only falls back to
		 * dismiss-then-reply for free text. */
		onsubmit: (text: string, picks: number[][] | null) => void;
	} = $props();

	// Per-question chosen option indices + free-text "Other". Seeded from
	// `questions` and RE-seeded whenever the prop's shape changes (CCT-350 item 1):
	// the live card instance is reused across successive asks, so without this the
	// arrays kept the previous ask's length/values — indexing into a stale slot
	// left `answeredAll` wrong and the answer un-submittable ("stuck pending").
	let chosen = $state<Set<number>[]>(questions.map(() => new Set<number>()));
	let other = $state<string[]>(questions.map(() => ''));
	// Which option's preview is shown per question (last hovered/selected).
	let focused = $state<number[]>(questions.map(() => 0));
	// Re-seed on a question-set change. Keyed on a cheap signature (count + the
	// joined question texts) so an identity-churn from a re-render that carries the
	// SAME questions doesn't wipe an in-progress selection.
	let lastSig = questions.map((q) => q.question).join('\0');
	$effect(() => {
		const sig = questions.map((q) => q.question).join('\0');
		if (sig === lastSig) return;
		lastSig = sig;
		chosen = questions.map(() => new Set<number>());
		other = questions.map(() => '');
		focused = questions.map(() => 0);
	});
	// Optimistic local lock (CCT-190): the card is fully prop-driven, so without
	// this it stays editable/"Send answer" until the server round-trip flips
	// `interactive` to false — a multi-second lag. Setting `submitted` on click
	// flips the card to its in-flight state instantly, independent of the server.
	let submitted = $state(false);
	// Editable only while interactive AND not yet submitted.
	const live = $derived(interactive && !submitted);
	// Release the optimistic lock if the parent re-enables the card (CCT-278):
	// `interactive` goes false while an answer is in flight and flips back to
	// true only if that answer failed to deliver (the parent clears its
	// `answering` lock). Detecting the false→true edge lets a failed answer be
	// resubmitted instead of staying stuck on "Answering…".
	let wasInteractive = interactive;
	$effect(() => {
		if (interactive && !wasInteractive) submitted = false;
		wasInteractive = interactive;
	});

	function pick(qi: number, oi: number) {
		if (!live) return;
		const q = questions[qi];
		const set = new Set(chosen[qi]);
		if (q.multiSelect) {
			if (set.has(oi)) set.delete(oi);
			else set.add(oi);
		} else {
			set.clear();
			set.add(oi);
		}
		chosen[qi] = set;
		focused[qi] = oi;
	}

	const answeredAll = $derived(
		questions.every((_, qi) => chosen[qi].size > 0 || other[qi].trim().length > 0)
	);

	function buildAnswer(): string {
		return questions
			.map((q, qi) => {
				const picks = [...chosen[qi]].map((oi) => q.options[oi]?.label).filter(Boolean);
				if (other[qi].trim()) picks.push(other[qi].trim());
				const head = q.header ? `**${q.header}** — ` : '';
				return `${head}${q.question}\n→ ${picks.join(', ')}`;
			})
			.join('\n\n');
	}

	/** Structured selection for the native answer path (CCT-226): one sorted
	 * list of 0-based option indices per question, or `null` if any question
	 * was answered (even partially) via the free-text "Other…" field. */
	function buildPicks(): number[][] | null {
		if (other.some((t) => t.trim().length > 0)) return null;
		return questions.map((_, qi) => [...chosen[qi]].sort((a, b) => a - b));
	}

	function submit() {
		if (!live || !answeredAll) return;
		submitted = true; // optimistic flip — show "Answering…" immediately
		onsubmit(buildAnswer(), buildPicks());
	}
</script>

<div class="ask" class:done={!live}>
	<Heading level={3} size="sm">{questions.length > 1 ? m.ask_questions_heading() : m.ask_question_heading()}</Heading>
	{#each questions as q, qi (qi)}
		{@const hasPreview = q.options.some((o) => o.preview)}
		<div class="q">
			<div class="q-top">
				{#if q.header}<Badge>{q.header}</Badge>{/if}
				<Text weight="medium">{q.question}</Text>
				{#if q.multiSelect}<Text tone="muted" size="xs">{m.ask_choose_any()}</Text>{/if}
			</div>
			<div class="q-body" class:split={hasPreview}>
				<div class="opts">
					{#each q.options as o, oi (oi)}
						<button
							type="button"
							class="opt"
							class:sel={chosen[qi].has(oi)}
							disabled={!live}
							onclick={() => pick(qi, oi)}
							onmouseenter={() => (focused[qi] = oi)}
						>
							<span class="mark">{chosen[qi].has(oi) ? (q.multiSelect ? '☑' : '◉') : q.multiSelect ? '☐' : '○'}</span>
							<span class="opt-text">
								<span class="opt-label">{o.label}</span>
								{#if o.description}<span class="opt-desc">{o.description}</span>{/if}
							</span>
						</button>
					{/each}
					<label class="opt other" class:sel={other[qi].trim().length > 0}>
						<span class="mark">✎</span>
						<input
							class="other-in"
							placeholder={m.ask_other_placeholder()}
							bind:value={other[qi]}
							disabled={!live}
						/>
					</label>
				</div>
				{#if hasPreview}
					<div class="preview">
						{#if q.options[focused[qi]]?.preview}
							<div class="preview-body">{@html renderMarkdown(q.options[focused[qi]].preview ?? '')}</div>
						{:else}
							<Text as="div" tone="muted" size="xs">{m.ask_no_preview()}</Text>
						{/if}
					</div>
				{/if}
			</div>
		</div>
	{/each}
	{#if live}
		<Button variant="primary" style="align-self:flex-start" disabled={!answeredAll} onclick={submit}>{m.ask_send_answer()}</Button>
	{:else if submitted && interactive}
		<Text as="div" class="answered" tone="muted" size="xs">{m.ask_answering()}</Text>
	{:else}
		<Text as="div" class="answered" tone="muted" size="xs">{m.ask_answered()}</Text>
	{/if}
</div>

<style>
	.ask {
		border: 1px solid var(--c-violet);
		border-radius: var(--r-md);
		background: color-mix(in srgb, var(--c-violet) 6%, var(--bg-elevated));
		padding: var(--sp-3);
		margin: var(--sp-2) 0;
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.ask.done {
		opacity: 0.7;
		border-color: var(--border);
		background: var(--bg-elevated-2);
	}
	.q {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.q-top {
		display: flex;
		align-items: baseline;
		flex-wrap: wrap;
		gap: var(--sp-2);
	}
	.q-body.split {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: var(--sp-3);
		align-items: start;
	}
	.opts {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.opt {
		display: flex;
		gap: var(--sp-2);
		align-items: flex-start;
		text-align: left;
		padding: var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
		color: var(--text);
		cursor: pointer;
		width: 100%;
	}
	.opt:hover:not(:disabled) {
		border-color: var(--c-violet);
	}
	.opt.sel {
		border-color: var(--c-violet);
		background: color-mix(in srgb, var(--c-violet) 14%, var(--bg-elevated-2));
	}
	.opt:disabled {
		cursor: default;
	}
	.mark {
		flex: 0 0 auto;
		font-size: var(--fs-sm);
	}
	.opt-text {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.opt-label {
		font-weight: 500;
	}
	.opt-desc {
		font-size: var(--fs-xs);
		color: var(--text-muted);
	}
	.opt.other {
		align-items: center;
	}
	.other-in {
		flex: 1;
		background: transparent;
		border: none;
		color: var(--text);
		outline: none;
	}
	.preview {
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg);
		padding: var(--sp-2);
		max-height: 320px;
		overflow: auto;
		font-family: var(--font-mono, monospace);
		font-size: var(--fs-xs);
	}
	.ask :global(.answered) {
		font-style: italic;
	}
</style>
