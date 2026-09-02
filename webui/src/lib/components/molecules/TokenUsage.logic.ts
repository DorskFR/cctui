import type { TokenUsage } from '@bindings/TokenUsage';

/** When Σ renders: always, only in the compact form (Σ + $ is that
 *  form, so a `showSum={false}` mount still needs it there), or never (no total). */
export type SumMode = 'always' | 'compact-only' | 'never';

export interface TokenUsageLayout {
	total: number;
	cacheTotal: number;
	cost: number;
	sumMode: SumMode;
	showCache: boolean;
	showCost: boolean;
	showCold: boolean;
}

export function tokenUsageLayout(
	usage: TokenUsage,
	{ sum = null, showSum = true, cold = false }: { sum?: number | null; showSum?: boolean; cold?: boolean } = {}
): TokenUsageLayout {
	const cacheTotal = Number(usage.cache_read_tokens) + Number(usage.cache_creation_tokens);
	const total = sum ?? Number(usage.tokens_in) + Number(usage.tokens_out) + cacheTotal;
	const cost = Number(usage.cost_usd) || 0;
	return {
		total,
		cacheTotal,
		cost,
		sumMode: total > 0 ? (showSum ? 'always' : 'compact-only') : 'never',
		showCache: cacheTotal > 0,
		showCost: cost > 0,
		showCold: cold
	};
}
