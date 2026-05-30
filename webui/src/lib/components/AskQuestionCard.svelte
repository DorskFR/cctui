<script lang="ts">
	import { renderMarkdown } from '$lib/markdown';

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
		onsubmit: (text: string) => void;
	} = $props();

	// Per-question chosen option indices + free-text "Other".
	let chosen = $state<Set<number>[]>(questions.map(() => new Set<number>()));
	let other = $state<string[]>(questions.map(() => ''));
	// Which option's preview is shown per question (last hovered/selected).
	let focused = $state<number[]>(questions.map(() => 0));

	function pick(qi: number, oi: number) {
		if (!interactive) return;
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

	function submit() {
		if (!interactive || !answeredAll) return;
		onsubmit(buildAnswer());
	}
</script>

<div class="ask" class:done={!interactive}>
	<div class="ask-head">❓ Question{questions.length > 1 ? 's' : ''}</div>
	{#each questions as q, qi (qi)}
		{@const hasPreview = q.options.some((o) => o.preview)}
		<div class="q">
			<div class="q-top">
				{#if q.header}<span class="chip">{q.header}</span>{/if}
				<span class="q-text">{q.question}</span>
				{#if q.multiSelect}<span class="muted sm">(choose any)</span>{/if}
			</div>
			<div class="q-body" class:split={hasPreview}>
				<div class="opts">
					{#each q.options as o, oi (oi)}
						<button
							type="button"
							class="opt"
							class:sel={chosen[qi].has(oi)}
							disabled={!interactive}
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
							placeholder="Other…"
							bind:value={other[qi]}
							disabled={!interactive}
						/>
					</label>
				</div>
				{#if hasPreview}
					<div class="preview">
						{#if q.options[focused[qi]]?.preview}
							<div class="preview-body">{@html renderMarkdown(q.options[focused[qi]].preview ?? '')}</div>
						{:else}
							<div class="muted sm">No preview for this option.</div>
						{/if}
					</div>
				{/if}
			</div>
		</div>
	{/each}
	{#if interactive}
		<button class="btn btn-primary submit" disabled={!answeredAll} onclick={submit}>Send answer</button>
	{:else}
		<div class="muted sm answered">Answered.</div>
	{/if}
</div>

<style>
	.ask {
		border: 1px solid var(--accent, #88c0d0);
		border-radius: var(--r-md);
		background: color-mix(in srgb, var(--accent, #88c0d0) 6%, var(--bg-elevated));
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
	.ask-head {
		font-weight: 600;
		font-size: var(--fs-sm);
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
	.q-text {
		font-weight: 500;
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
		border-color: var(--accent, #88c0d0);
	}
	.opt.sel {
		border-color: var(--accent, #88c0d0);
		background: color-mix(in srgb, var(--accent, #88c0d0) 14%, var(--bg-elevated-2));
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
	.submit {
		align-self: flex-start;
	}
	.answered {
		font-style: italic;
	}
</style>
