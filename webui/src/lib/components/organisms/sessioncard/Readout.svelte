<script lang="ts">
	import AdapterIcon from '#lib/components/atoms/AdapterIcon.svelte';
	import { tokenUsageLayout } from '#lib/components/molecules/TokenUsage.logic.ts';
	import { modelAbbrev, modelFamily, modelShort, usd } from '#lib/format.ts';
	import { Text } from '@dorsk/tsumikit';
	import TokenTotal from './TokenTotal.svelte';
	import type { SessionView } from './view';

	// tokens · cost · model · effort · adapter logo. As the container tightens the
	// cost goes, then the model sheds its effort, its version ("opus"), all but two
	// letters ("Op."), and the adapter logo. The title carries the full id.
	let {
		view,
		spread = false
	}: {
		view: SessionView;
		/** Push the model and logo to the far end of the line. */
		spread?: boolean;
	} = $props();

	const s = $derived(view.s);
	const sum = $derived(view.rollup ? view.rollup.tokens : null);
	const usage = $derived(tokenUsageLayout(s.token_usage, { sum }));
	const modelTitle = $derived(
		s.model ? `${modelShort(s.model)}${s.effort ? ` · ${s.effort}` : ''}` : ''
	);
</script>

{#if !view.draft}
	<Text tone="muted" size="xs"><TokenTotal usage={s.token_usage} {sum} cold={s.cache_cold} /></Text>
	{#if usage.showCost}<span class="cost"><Text tone="muted" size="xs" numeric>{usd(usage.cost)}</Text></span>{/if}
{/if}
{#if spread}<span class="gap"></span>{/if}
{#if s.model}
	<span class="model" title={modelTitle}>
		<Text tone="muted" size="xs" style="white-space:nowrap">
			<span class="full">{modelShort(s.model)}</span><span class="fam">{modelFamily(s.model)}</span
			><span class="abbr">{modelAbbrev(s.model)}</span
			>{#if s.effort}<span class="effort">{` · ${s.effort}`}</span>{/if}
		</Text>
	</span>
{/if}
<span class="logo"><AdapterIcon adapter={s.adapter_id} size={14} /></span>

<style>
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
	.cost {
		flex: none;
		display: inline-flex;
	}
	.fam,
	.abbr {
		display: none;
	}
	@container sess-card (max-width: 17em) {
		.cost {
			display: none;
		}
	}
	@container sess-row (max-width: 21.25em) {
		.cost {
			display: none;
		}
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
	@container sess-row (max-width: 42.75em) {
		.effort {
			display: none;
		}
	}
	@container sess-row (max-width: 36.25em) {
		.full {
			display: none;
		}
		.fam {
			display: inline;
		}
	}
	@container sess-row (max-width: 20em) {
		.model {
			display: none;
		}
	}
	@container sess-row (max-width: 32em) {
		.fam {
			display: none;
		}
		.abbr {
			display: inline;
		}
		.logo {
			display: none;
		}
	}
</style>
