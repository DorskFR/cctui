<script lang="ts">
	import { Badge, Card, Text } from '@dorsk/tsumikit';
	import type { OutcomeStatus } from '@bindings/OutcomeStatus';
	import type { SessionOutcome } from '@bindings/SessionOutcome';
	import { renderMarkdown } from '$lib/markdown';
	import { outcomeTone } from '$lib/outcome';
	import { m } from '$lib/paraglide/messages';

	let { outcome }: { outcome: SessionOutcome | null | undefined } = $props();

	const LABEL: Record<OutcomeStatus, () => string> = {
		result: m.outcome_status_result,
		needs_input: m.outcome_status_needs_input,
		failed: m.outcome_status_failed,
		ended_without_outcome: m.outcome_status_ended_without_outcome
	};

	const bodyHtml = $derived(outcome?.body ? renderMarkdown(outcome.body) : null);
	const isLink = (e: string) => /^https?:\/\//.test(e);
</script>

{#if outcome && outcome.source !== 'inferred'}
	<Card
		tone="neutral"
		surface="raised"
		padding="sm"
		gap="var(--sp-2)"
		style="margin:var(--sp-2) var(--sp-3)"
		aria-label={m.outcome_label()}
	>
		<div class="head">
			<Badge tone={outcomeTone(outcome.status)} size="xs">{LABEL[outcome.status]()}</Badge>
			<Text weight="medium" size="sm" truncate title={outcome.headline}>{outcome.headline}</Text>
		</div>
		{#if bodyHtml}
			<div class="body">{@html bodyHtml}</div>
		{/if}
		{#if outcome.evidence?.length}
			<ul class="evidence" aria-label={m.outcome_evidence()}>
				{#each outcome.evidence as e (e)}
					<li>
						{#if isLink(e)}<a href={e} target="_blank" rel="noreferrer noopener">{e}</a>{:else}<code
								>{e}</code
							>{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
{/if}

<style>
	.head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.body {
		font-size: var(--fs-xs);
		color: var(--text-muted);
		overflow-wrap: anywhere;
	}
	.evidence {
		margin: 0;
		padding-left: var(--sp-4);
		font-size: var(--fs-xs);
		overflow-wrap: anywhere;
	}
</style>
