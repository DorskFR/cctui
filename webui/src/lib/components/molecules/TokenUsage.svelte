<script lang="ts">
	import type { TokenUsage } from '@bindings/TokenUsage';
	import { compact } from '$lib/format';

	// Compact token-usage readout (↑in ↓out ⚡cache), shared by the session
	// list card and the chat header.
	// `cold` (CCT-189): the session's last turn re-billed the full context
	// (prompt cache had gone cold) → show a ❄️ next to the counts.
	let { usage, cold = false }: { usage: TokenUsage; cold?: boolean } = $props();
</script>

<span class="tokens mono faint">
	↑{compact(Number(usage.tokens_in))} ↓{compact(Number(usage.tokens_out))}
	{#if Number(usage.cache_read_tokens) > 0}⚡{compact(Number(usage.cache_read_tokens))}{/if}
	{#if cold}<span class="cold" title="Cache went cold — the next send re-bills the full context">❄️</span>{/if}
</span>

<style>
	.tokens {
		font-size: var(--fs-xs);
	}
	.cold {
		font-size: 0.85em;
		cursor: help;
	}
</style>
