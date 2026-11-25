<script lang="ts">
	import type { TokenUsage } from '@bindings/TokenUsage';
	import { tokenUsageLayout } from '#lib/components/molecules/TokenUsage.logic.ts';
	import { compact } from '#lib/format.ts';
	import { m } from '#lib/paraglide/messages.js';

	let { usage, sum = null }: { usage: TokenUsage; sum?: number | null } = $props();

	const layout = $derived(tokenUsageLayout(usage, { sum }));
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
	<span class="tt" {title}>{m.sessions_row_tokens({ count: layout.total, tokens: compact(layout.total) })}</span>
{/if}

<style>
	.tt {
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
</style>
