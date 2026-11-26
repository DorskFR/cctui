<script lang="ts">
	import type { TokenUsage } from '@bindings/TokenUsage';
	import { tokenUsageLayout } from '#lib/components/molecules/TokenUsage.logic.ts';
	import { compact } from '#lib/format.ts';
	import { m } from '#lib/paraglide/messages.js';

	let {
		usage,
		sum = null,
		cold = false
	}: { usage: TokenUsage; sum?: number | null; cold?: boolean } = $props();

	const layout = $derived(tokenUsageLayout(usage, { sum, cold }));
	const title = $derived(
		m.sessions_row_tokens_title({
			total: layout.total.toLocaleString(),
			input: compact(Number(usage.tokens_in)),
			output: compact(Number(usage.tokens_out)),
			cache: compact(layout.cacheTotal)
		})
	);
</script>

{#if layout.total > 0}
	<span class="tt" {title}>{m.sessions_row_tokens({ count: layout.total, tokens: compact(layout.total) })}</span
	>{#if layout.showCold}<span class="cold" role="img" aria-label={m.sessions_token_cold_hint()} title={m.sessions_token_cold_hint()}
			>❄️</span
		>{/if}
{/if}

<style>
	.cold {
		margin-inline-start: 0.25em;
		font-size: 0.85em;
		cursor: help;
	}
	.tt {
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
</style>
