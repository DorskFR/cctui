import { describe, expect, it } from 'vitest';
import type { UsagePace, UsageWindow } from '$lib/queries';
import {
	barPct,
	countdown,
	headroomTone,
	paceState,
	wallInMs
} from '$lib/components/molecules/usage-battery.logic';
import {
	money,
	resetIn,
	resetInShort,
	usdPct,
	usdReadout
} from '$lib/components/molecules/cap-bar.logic';
import { parityFixture } from './fixtures';

type Fixture = {
	headroomTone: { utilization: number | null; out: string }[];
	paceState: { ratio: number | null; out: string | null }[];
	countdown: { ms: number; out: string }[];
	resetIn: { resetsAtMs: number | null; nowMs: number; out: string | null }[];
	resetInShort: { resetsAtMs: number | null; nowMs: number; out: string | null }[];
	usdPct: { amountUsd: number | null; capUsd: number | null; out: number | null }[];
	usdReadout: { amountUsd: number | null; capUsd: number | null; out: string | null }[];
	money: { n: number; out: string }[];
	barPct: { utilization: number | null; out: number | null }[];
	wallInMs: {
		wallAtMs: number | null;
		resetsAtMs: number | null;
		nowMs: number;
		out: number | null;
	}[];
};

const fx = parityFixture<Fixture>('usage');

/** The fixture speaks unix ms so the Rust port can read it; the TS originals
 *  take the rfc3339 strings the API sends. */
const iso = (ms: number | null) => (ms === null ? null : new Date(ms).toISOString());

const pace = (over: Partial<UsagePace>): UsagePace =>
	({ elapsed_fraction: 0, expected_pct: 0, ratio: 0, ...over }) as UsagePace;

const window = (utilization: number | null): UsageWindow =>
	({
		key: 'session',
		kind: 'session',
		label: '5h',
		utilization: utilization ?? Number.NaN
	}) as UsageWindow;

describe('usage parity fixtures', () => {
	it('headroomTone', () => {
		for (const c of fx.headroomTone)
			expect(headroomTone(c.utilization), JSON.stringify(c)).toBe(c.out);
	});
	it('paceState', () => {
		for (const c of fx.paceState)
			expect(
				paceState(c.ratio === null ? null : pace({ ratio: c.ratio })),
				JSON.stringify(c)
			).toBe(c.out);
	});
	it('countdown', () => {
		for (const c of fx.countdown) expect(countdown(c.ms), JSON.stringify(c)).toBe(c.out);
	});
	it('resetIn', () => {
		for (const c of fx.resetIn)
			expect(resetIn(iso(c.resetsAtMs), c.nowMs), JSON.stringify(c)).toBe(c.out);
	});
	it('resetInShort', () => {
		for (const c of fx.resetInShort)
			expect(resetInShort(iso(c.resetsAtMs), c.nowMs), JSON.stringify(c)).toBe(c.out);
	});
	it('usdPct', () => {
		for (const c of fx.usdPct)
			expect(usdPct(c.amountUsd, c.capUsd), JSON.stringify(c)).toBe(c.out);
	});
	it('usdReadout', () => {
		for (const c of fx.usdReadout)
			expect(usdReadout(c.amountUsd, c.capUsd), JSON.stringify(c)).toBe(c.out);
	});
	it('money', () => {
		for (const c of fx.money) expect(money(c.n), JSON.stringify(c)).toBe(c.out);
	});
	it('barPct', () => {
		for (const c of fx.barPct)
			expect(barPct(c.utilization === null ? null : window(c.utilization)), JSON.stringify(c)).toBe(
				c.out
			);
	});
	it('wallInMs', () => {
		for (const c of fx.wallInMs)
			expect(
				wallInMs(
					c.wallAtMs === null ? null : pace({ projected_wall_at: iso(c.wallAtMs) ?? undefined }),
					iso(c.resetsAtMs),
					c.nowMs
				),
				JSON.stringify(c)
			).toBe(c.out);
	});
});
