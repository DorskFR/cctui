<script lang="ts">
	import type { TokenUsage } from '@bindings/TokenUsage';
	import { compact } from '$lib/format';
	import Text from '$lib/components/atoms/Text.svelte';

	// Compact token-usage readout (↑in ↓out ⚡cache), shared by the session
	// list card and the chat header.
	// `cold` (CCT-189): the session's last turn re-billed the full context
	// (prompt cache had gone cold) → show a ❄️ next to the counts.
	let { usage, cold = false }: { usage: TokenUsage; cold?: boolean } = $props();
</script>

<Text variant="code" size="xs" tone="faint">
	↑{compact(Number(usage.tokens_in))} ↓{compact(Number(usage.tokens_out))}
	{#if Number(usage.cache_read_tokens) > 0}⚡{compact(Number(usage.cache_read_tokens))}{/if}
	{#if cold}<Text class="cold" title="Cache went cold — the next send re-bills the full context">❄️</Text>{/if}
</Text>

<style>
	/* .cold is rendered by the Text atom, so this chrome must be :global to reach
	   it. .tokens carries no residual style (Text owns size/tone/mono). */
	:global(.cold) {
		font-size: 0.85em;
		cursor: help;
	}
</style>
