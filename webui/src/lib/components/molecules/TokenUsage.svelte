<script lang="ts">
	import type { TokenUsage } from '@bindings/TokenUsage';
	import { compact } from '$lib/format';
	import { Cluster, Text } from '@dorsk/tsumikit';

	// Canonical token-usage readout — the SINGLE token block, shared by the session
	// list/card, the chat header, and each assistant/result line in the conversation.
	// Renders:  Σtotal · ↑in ↓out ⚡cache (❄️)
	//   • Σ leads with the total (the headline number).
	//   • `sum` overrides Σ: the card passes the parent+subagents aggregate so Σ
	//     reflects the true cost-including-subagents; everywhere else Σ defaults to
	//     this block's OWN total (in + out + cache read + cache creation).
	//   • `cold` (CCT-189): the last turn re-billed the full context (prompt cache
	//     went cold) → a ❄️ next to the counts.
	//   • `showSum`: per-message conversation lines want only the reply's own
	//     breakdown (↑↓⚡), no leading Σ — they pass showSum={false}.
	//   • `wrap`: false (default) keeps the readout on one line — the list/header/
	//     lines never want it to break; the overview stats pass wrap so the larger
	//     `size` can fold inside a narrow card instead of spilling.
	// `size` rides through to each Text segment — defaults to the compact `xs`.
	let {
		usage,
		cold = false,
		sum = null,
		showSum = true,
		size = 'xs',
		wrap = false
	}: {
		usage: TokenUsage;
		cold?: boolean;
		sum?: number | null;
		showSum?: boolean;
		size?: 'xs' | 'sm' | 'base' | 'md' | 'lg' | 'xl' | '2xl';
		wrap?: boolean;
	} = $props();

	const cacheTotal = $derived(
		Number(usage.cache_read_tokens) + Number(usage.cache_creation_tokens)
	);
	const total = $derived(sum ?? Number(usage.tokens_in) + Number(usage.tokens_out) + cacheTotal);
</script>

<!-- Cluster owns the layout (row, single gap, optional wrap); each segment is its
     own Text atom, carrying tone/weight/size as props rather than CSS overrides. -->
<Cluster gap="0.4rem" align="baseline" {wrap}>
	{#if showSum && total > 0}<Text
			variant="code"
			{size}
			tone="accent"
			weight="semibold"
			style="cursor:help"
			title="Σ — cumulative session usage (↑ + ↓ + ⚡), summed over every turn. This is lifetime billing-style throughput, NOT the current context size: ⚡ re-counts the cached context on each turn so Σ climbs well past what /context reports as loaded right now."
			>Σ{compact(total)}</Text
		>{/if}
	<Text variant="code" {size} tone="faint" style="cursor:help" title="↑ — new (uncached) input tokens, summed over the session. Small because each turn re-sends the bulk of the context as a cache read (⚡), not fresh input.">↑{compact(Number(usage.tokens_in))}</Text>
	<Text variant="code" {size} tone="faint" style="cursor:help" title="↓ — output (generated) tokens, summed over the session.">↓{compact(Number(usage.tokens_out))}</Text>
	{#if cacheTotal > 0}<Text variant="code" {size} tone="faint" style="cursor:help" title="⚡ — cache read + create tokens, counted every turn. The whole context is re-read from cache each turn, so this accumulates ≈ window size × turns and dominates Σ. It is cumulative throughput, not current context occupancy.">⚡{compact(cacheTotal)}</Text>{/if}
	{#if cold}<Text
			variant="code"
			{size}
			tone="faint"
			title="Cache went cold — the next send re-bills the full context"
			style="font-size: 0.85em; cursor: help">❄️</Text
		>{/if}
</Cluster>
