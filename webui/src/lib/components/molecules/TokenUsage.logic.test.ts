import { describe, expect, it } from 'vitest';
import type { TokenUsage } from '@bindings/TokenUsage';
import { tokenUsageLayout } from './TokenUsage.logic';

const usage = (over: Partial<TokenUsage> = {}): TokenUsage =>
	({
		tokens_in: 1000,
		tokens_out: 200,
		cache_read_tokens: 5000,
		cache_creation_tokens: 300,
		cost_usd: 1.25,
		...over
	}) as unknown as TokenUsage;

describe('tokenUsageLayout', () => {
	it('sums in + out + cache for Σ and shows every segment by default', () => {
		const l = tokenUsageLayout(usage());
		expect(l.total).toBe(6500);
		expect(l.cacheTotal).toBe(5300);
		expect(l.cost).toBe(1.25);
		expect(l.sumMode).toBe('always');
		expect(l.showCache).toBe(true);
		expect(l.showCost).toBe(true);
		expect(l.showCold).toBe(false);
	});

	it('lets `sum` override Σ without touching the breakdown', () => {
		const l = tokenUsageLayout(usage(), { sum: 99_000 });
		expect(l.total).toBe(99_000);
		expect(l.cacheTotal).toBe(5300);
	});

	it('keeps Σ for the compact form when showSum is false', () => {
		expect(tokenUsageLayout(usage(), { showSum: false }).sumMode).toBe('compact-only');
	});

	it('never renders Σ without a total, whatever showSum says', () => {
		const empty = usage({ tokens_in: 0, tokens_out: 0, cache_read_tokens: 0, cache_creation_tokens: 0 });
		expect(tokenUsageLayout(empty).sumMode).toBe('never');
		expect(tokenUsageLayout(empty, { showSum: false }).sumMode).toBe('never');
		expect(tokenUsageLayout(usage(), { sum: 0 }).sumMode).toBe('never');
	});

	it('hides cache, cost and cold when they carry nothing', () => {
		const l = tokenUsageLayout(usage({ cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0 }));
		expect(l.showCache).toBe(false);
		expect(l.showCost).toBe(false);
		expect(l.total).toBe(1200);
		expect(tokenUsageLayout(usage({ cost_usd: null as unknown as number })).cost).toBe(0);
		expect(tokenUsageLayout(usage(), { cold: true }).showCold).toBe(true);
	});
});
