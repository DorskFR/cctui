<script lang="ts">
	import type { TokenUsage } from '@bindings/TokenUsage';
	import { compact } from '$lib/format';

	// Compact token-usage readout (↑in ↓out ⚡cache), shared by the session
	// list card and the chat header.
	let { usage }: { usage: TokenUsage } = $props();
</script>

<span class="tokens mono faint">
	↑{compact(Number(usage.tokens_in))} ↓{compact(Number(usage.tokens_out))}
	{#if Number(usage.cache_read_tokens) > 0}⚡{compact(Number(usage.cache_read_tokens))}{/if}
</span>

<style>
	.tokens {
		font-size: var(--fs-xs);
	}
</style>
