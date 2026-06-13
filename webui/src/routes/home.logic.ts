// Pure helpers for the overview page — no Svelte/reactive state, so they live
// outside the component and are unit-testable on their own.
import type { WindowTokenUsage } from '@bindings/WindowTokenUsage';
import type { TokenUsage } from '@bindings/TokenUsage';

// Adapt a window's {input, output, cache_read} to the shape TokenUsage.svelte
// renders (the session-card readout). cost_usd / cache_creation are unused here.
export const asUsage = (w: WindowTokenUsage | undefined): TokenUsage => ({
	tokens_in: w?.input ?? 0,
	tokens_out: w?.output ?? 0,
	cost_usd: 0,
	cache_read_tokens: w?.cache_read ?? 0,
	cache_creation_tokens: 0
});
