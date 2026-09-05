<script lang="ts">
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import { modelFamily, modelShort } from '$lib/format';
	import { Text } from '@dorsk/tsumikit';
	import type { SessionView } from './view';

	// Σ ↑ ↓ ⚡ $ · model · effort · adapter logo. Inside a cramped `sess-card`
	// container the model drops its effort, then falls back to the family.
	let {
		view,
		compact = false
	}: {
		view: SessionView;
		/** Σ + $ only (the compact row). */
		compact?: boolean;
	} = $props();

	const s = $derived(view.s);
	const modelTitle = $derived(
		s.model ? `${modelShort(s.model)}${s.effort ? ` · ${s.effort}` : ''}` : ''
	);
</script>

{#if !view.draft}
	<TokenUsage usage={s.token_usage} cold={s.cache_cold} sum={view.rollup ? view.rollup.tokens : null} {compact} />
{/if}
{#if s.model}
	<span class="model" title={modelTitle}>
		<Text tone="muted" size="xs" style="white-space:nowrap">
			<span class="full">{modelShort(s.model)}</span><span class="fam">{modelFamily(s.model)}</span
			>{#if s.effort}<span class="effort"> · {s.effort}</span>{/if}
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
	.fam {
		display: none;
	}
	@container sess-card (max-width: 40rem) {
		.effort {
			display: none;
		}
	}
	@container sess-card (max-width: 34rem) {
		.full {
			display: none;
		}
		.fam {
			display: inline;
		}
	}
</style>
