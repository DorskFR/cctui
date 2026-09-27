import { describe, it, expect } from 'vitest';
import type { DailyCacheLoss } from '$lib/queries';
import { cacheLossRows, cacheLossTotals } from './cache-loss';

const day = (d: string, ttl: number, gw: number, unk: number, usd = 0): DailyCacheLoss => ({
	day: d,
	ttl_expired: 0,
	gateway_rewrote_body: 0,
	unknown: 0,
	total: usd,
	ttl_expired_tokens: ttl,
	gateway_rewrote_body_tokens: gw,
	unknown_tokens: unk,
	lost_tokens: ttl + gw + unk,
	busts: 1
});

describe('cacheLossRows', () => {
	it('orders newest first and scales every reason against the day that lost the most tokens', () => {
		const rows = cacheLossRows([day('2026-09-19', 1, 0, 1), day('2026-09-20', 2, 1, 1)]);
		expect(rows.map((r) => r.day)).toEqual(['2026-09-20', '2026-09-19']);
		expect(rows[0].widths).toEqual({ ttl_expired: 50, gateway_rewrote_body: 25, unknown: 25 });
		expect(rows[1].widths).toEqual({ ttl_expired: 25, gateway_rewrote_body: 0, unknown: 25 });
	});

	it('draws bars from tokens even when no bust could be priced', () => {
		const [row] = cacheLossRows([day('2026-09-20', 0, 0, 172_523)]);
		expect(row.widths.unknown).toBe(100);
		expect(row.tokens).toBe(172_523);
		expect(row.usd).toBe(0);
	});

	it('draws empty bars when nothing was lost', () => {
		expect(cacheLossRows([day('2026-09-20', 0, 0, 0)])[0].widths.ttl_expired).toBe(0);
		expect(cacheLossRows([])).toEqual([]);
	});
});

describe('cacheLossTotals', () => {
	it('sums tokens per reason, dollars and busts over the range', () => {
		expect(
			cacheLossTotals([day('2026-09-19', 1, 0, 1, 0.5), day('2026-09-20', 2, 1, 1, 0.25)])
		).toEqual({
			tokens: { ttl_expired: 3, gateway_rewrote_body: 1, unknown: 2, total: 6 },
			usd: 0.75,
			busts: 2
		});
	});
});
