<script lang="ts">
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import StatText from '$lib/components/atoms/StatText.svelte';
	import { tokenUsageLayout } from '$lib/components/molecules/TokenUsage.logic';
	import { modelAbbrev, modelFamily, modelShort } from '$lib/format';
	import { Text } from '@dorsk/tsumikit';
	import type { SessionView } from './view';

	// tokens · cost · model · effort · adapter logo. As the container tightens the
	// model sheds its effort, then its version ("opus"), then all but two letters
	// ("Op.").
	let {
		view,
		spread = false
	}: {
		view: SessionView;
		spread?: boolean;
	} = $props();

	const s = $derived(view.s);
	const usage = $derived(
		tokenUsageLayout(s.token_usage, { sum: view.rollup ? view.rollup.tokens : null })
	);
	const modelTitle = $derived(
		s.model ? `${modelShort(s.model)}${s.effort ? ` · ${s.effort}` : ''}` : ''
	);
</script>

{#if !view.draft && usage.total > 0}
	<span class="stat"><StatText value={usage.total} kind="tokens" compact /></span>
	{#if usage.showCost}<span class="stat cost"><StatText value={usage.cost} kind="cost" /></span>{/if}
{/if}
{#if spread}<span class="gap"></span>{/if}
{#if s.model}
	<span class="model" title={modelTitle}>
		<Text tone="muted" size="xs" style="white-space:nowrap">
			<span class="full">{modelShort(s.model)}</span><span class="fam">{modelFamily(s.model)}</span
			><span class="abbr">{modelAbbrev(s.model)}</span
			>{#if s.effort}<span class="effort"> · {s.effort}</span>{/if}
		</Text>
	</span>
{/if}
<span class="logo"><AdapterIcon adapter={s.adapter_id} size={14} /></span>

<style>
	.stat {
		flex: none;
		display: inline-flex;
		font-size: var(--fs-xs);
		color: var(--text-muted);
		white-space: nowrap;
	}
	@container sess-card (max-width: 17em) {
		.cost {
			display: none;
		}
	}
	.model {
		flex: none;
		display: inline-flex;
		min-width: 0;
	}
	.logo {
		flex: none;
		display: inline-flex;
	}
	.gap {
		flex: 1 1 0;
	}
	.fam,
	.abbr {
		display: none;
	}
	@container sess-card (max-width: 27.75em) {
		.effort {
			display: none;
		}
	}
	@container sess-card (max-width: 17em) {
		.full {
			display: none;
		}
		.fam {
			display: inline;
		}
	}
	@container sess-card (max-width: 15em) {
		.fam {
			display: none;
		}
		.abbr {
			display: inline;
		}
	}
</style>
