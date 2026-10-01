import { describe, expect, it } from 'vitest';
import {
	fillDaily,
	hasLangfuseCost,
	langfuseCostLabel,
	modelSpend,
	spendSince,
	spendTotals,
	type ModelSpendRow,
	type SessionSpend,
	type Windows
} from '$lib/spend';
import { cacheLossTotals } from '$lib/components/organisms/overview/cache-loss';
import type { DailyCacheLoss } from '$lib/queries';
import { parityFixture } from './fixtures';

type Fixture = {
	spendSince: { sessions: SessionSpend[]; cutoffMs: number; out: number }[];
	modelSpend: {
		sessions: SessionSpend[];
		windows: Windows;
		out: ModelSpendRow[];
		totals: ModelSpendRow;
	}[];
	fillDaily: {
		points: { dayMs: number; tokens: number }[];
		days: number;
		endDayMs: number;
		out: number[];
	}[];
	cacheLossTotals: {
		days: DailyCacheLoss[];
		out: { usd: number; tokens: number; busts: number };
	}[];
	langfuseCostLabel: { costUsd: number; out: string }[];
	hasLangfuseCost: { usage: { cost_usd: number; trace_count: number } | null; out: boolean }[];
};

const fx = parityFixture<Fixture>('spend');

describe('spend parity', () => {
	it('spendSince', () => {
		for (const c of fx.spendSince) {
			expect(spendSince(c.sessions, c.cutoffMs)).toBeCloseTo(c.out, 9);
		}
	});

	it('modelSpend + spendTotals', () => {
		for (const c of fx.modelSpend) {
			const rows = modelSpend(c.sessions, c.windows);
			expect(rows.map((r) => r.model)).toEqual(c.out.map((r) => r.model));
			rows.forEach((row, i) => {
				expect(row.today).toBeCloseTo(c.out[i].today, 9);
				expect(row.week).toBeCloseTo(c.out[i].week, 9);
				expect(row.month).toBeCloseTo(c.out[i].month, 9);
			});
			const totals = spendTotals(rows);
			expect(totals.model).toBe(c.totals.model);
			expect(totals.today).toBeCloseTo(c.totals.today, 9);
			expect(totals.week).toBeCloseTo(c.totals.week, 9);
			expect(totals.month).toBeCloseTo(c.totals.month, 9);
		}
	});

	it('fillDaily', () => {
		for (const c of fx.fillDaily) {
			expect(fillDaily(c.points, c.days, c.endDayMs)).toEqual(c.out);
		}
	});

	it('cacheLossTotals', () => {
		for (const c of fx.cacheLossTotals) {
			const totals = cacheLossTotals(c.days);
			expect(totals.usd).toBeCloseTo(c.out.usd, 9);
			expect(totals.tokens.total).toBe(c.out.tokens);
			expect(totals.busts).toBe(c.out.busts);
		}
	});

	it('langfuseCostLabel', () => {
		for (const c of fx.langfuseCostLabel) {
			expect(langfuseCostLabel(c.costUsd)).toBe(c.out);
		}
	});

	it('hasLangfuseCost', () => {
		for (const c of fx.hasLangfuseCost) {
			expect(hasLangfuseCost(c.usage)).toBe(c.out);
		}
	});
});
