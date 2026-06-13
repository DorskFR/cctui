<script lang="ts">
	import type { TokenUsage } from '@bindings/TokenUsage';
	import { compact } from '$lib/format';
	import { Text } from '@dorsk/tsumikit';

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
	let {
		usage,
		cold = false,
		sum = null,
		showSum = true
	}: { usage: TokenUsage; cold?: boolean; sum?: number | null; showSum?: boolean } = $props();

	const cacheTotal = $derived(
		Number(usage.cache_read_tokens) + Number(usage.cache_creation_tokens)
	);
	const total = $derived(sum ?? Number(usage.tokens_in) + Number(usage.tokens_out) + cacheTotal);
</script>

<Text variant="code" size="xs" tone="faint" class="tok-usage">
	{#if showSum && total > 0}<span class="tok-sum">Σ{compact(total)}</span>{/if}
	<span>↑{compact(Number(usage.tokens_in))}</span>
	<span>↓{compact(Number(usage.tokens_out))}</span>
	{#if cacheTotal > 0}<span>⚡{compact(cacheTotal)}</span>{/if}
	{#if cold}<span class="cold" title="Cache went cold — the next send re-bills the full context"
			>❄️</span
		>{/if}
</Text>

<style>
	/* Flex container so the segments (Σ · ↑ · ↓ · ⚡ · ❄️) are spaced by a single
	   gap — no whitespace fiddling. Each ↑/↓/⚡ keeps its glyph glued to its number. */
	:global(.tok-usage) {
		display: inline-flex;
		align-items: baseline;
		gap: 0.4rem;
	}
	/* Σ is the headline: accent + semibold. */
	.tok-sum {
		font-weight: var(--fw-semibold);
		color: var(--accent);
	}
	.cold {
		font-size: 0.85em;
		cursor: help;
	}
</style>
