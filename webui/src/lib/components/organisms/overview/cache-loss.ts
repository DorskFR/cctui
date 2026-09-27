import type { DailyCacheLoss } from '$lib/queries';

export const CACHE_LOSS_DAYS = 7;

export const CACHE_LOSS_REASONS = ['ttl_expired', 'gateway_rewrote_body', 'unknown'] as const;
export type CacheLossReason = (typeof CACHE_LOSS_REASONS)[number];

const tokensOf = (d: DailyCacheLoss, r: CacheLossReason): number => d[`${r}_tokens`];

export interface CacheLossRow {
	day: string;
	tokens: number;
	usd: number;
	busts: number;
	/** Each reason's share of the widest day's bar, in percent. */
	widths: Record<CacheLossReason, number>;
}

/** Newest day first, bars scaled against the day that lost the most tokens. */
export function cacheLossRows(days: DailyCacheLoss[]): CacheLossRow[] {
	const peak = Math.max(0, ...days.map((d) => d.lost_tokens));
	return [...days]
		.sort((a, b) => b.day.localeCompare(a.day))
		.map((d) => ({
			day: d.day,
			tokens: d.lost_tokens,
			usd: d.total,
			busts: d.busts,
			widths: Object.fromEntries(
				CACHE_LOSS_REASONS.map((r) => [r, peak > 0 ? (tokensOf(d, r) / peak) * 100 : 0])
			) as Record<CacheLossReason, number>
		}));
}

export interface CacheLossTotals {
	tokens: Record<CacheLossReason | 'total', number>;
	usd: number;
	busts: number;
}

/** Range totals: lost tokens per reason and overall, dollars, bust count. */
export function cacheLossTotals(days: DailyCacheLoss[]): CacheLossTotals {
	const tokens = { ttl_expired: 0, gateway_rewrote_body: 0, unknown: 0, total: 0 };
	let usd = 0;
	let busts = 0;
	for (const d of days) {
		for (const r of CACHE_LOSS_REASONS) tokens[r] += tokensOf(d, r);
		tokens.total += d.lost_tokens;
		usd += d.total;
		busts += d.busts;
	}
	return { tokens, usd, busts };
}
