import { describe, expect, it } from 'vitest';
import type { UsageWindow } from '$lib/queries';
import { editorWindowKeys, isUsdKey, mergeUsageWindows, windowLabelFromKey } from './usage-windows';

const win = (key: string, label: string, utilization: number, extra: Partial<UsageWindow> = {}): UsageWindow => ({
	key,
	kind: key === 'session' ? 'session' : key.startsWith('weekly_model:') ? 'weekly_scoped' : 'weekly_all',
	label,
	utilization,
	...extra
});

describe('mergeUsageWindows pace', () => {
	it('carries the server pace onto observed rows and nulls it on unobserved ones', () => {
		const pace = { elapsed_fraction: 0.5, expected_pct: 50, ratio: 1.4, projected_wall_at: null };
		const rows = mergeUsageWindows([win('session', '5h', 70, { pace })], { weekly_all: { cap_pct: 80 } });
		expect(rows.observed[0].pace).toEqual(pace);
		expect(rows.unobserved[0].pace).toBeNull();
	});
});

describe('windowLabelFromKey', () => {
	it('labels the canonical keys and falls back for model-scoped', () => {
		expect(windowLabelFromKey('session')).toBe('5h');
		expect(windowLabelFromKey('weekly_all')).toBe('7d');
		expect(windowLabelFromKey('weekly_model:fable')).toBe('fable');
	});
});

describe('mergeUsageWindows', () => {
	it('renders a structured multi-window response (session + weekly_all + weekly_model)', () => {
		const windows = [
			win('session', '5h', 40),
			win('weekly_all', 'Weekly (all models)', 55),
			win('weekly_model:fable', 'Weekly Fable', 12, { model_id: 'fable-1' })
		];
		const { observed, unobserved } = mergeUsageWindows(windows, { session: { cap_pct: 80 } });
		expect(observed.map((r) => r.key)).toEqual(['session', 'weekly_all', 'weekly_model:fable']);
		expect(observed[0].cap).toBe(80);
		// The server's long label never wins over the short canonical one.
		expect(observed[1].label).toBe('7d');
		expect(observed[2].label).toBe('fable');
		expect(observed[2].utilization).toBe(12);
		expect(unobserved).toHaveLength(0);
	});

	it('weekly-only response still yields rows (never the "no usage data" empty state)', () => {
		const { observed, unobserved } = mergeUsageWindows([win('weekly_all', 'Weekly (all models)', 30)], null);
		expect(observed).toHaveLength(1);
		expect(observed[0].key).toBe('weekly_all');
		expect(unobserved).toHaveLength(0);
		expect(observed.length + unobserved.length).toBeGreaterThan(0);
	});

	it('surfaces a configured-but-unobserved key separately so it stays editable', () => {
		const { observed, unobserved } = mergeUsageWindows([win('session', '5h', 20)], {
			session: { cap_pct: 90 },
			'weekly_model:gone': { cap_pct: 50, bypass_minutes: 120 }
		});
		expect(observed.map((r) => r.key)).toEqual(['session']);
		expect(unobserved.map((r) => r.key)).toEqual(['weekly_model:gone']);
		expect(unobserved[0].utilization).toBeNull();
		expect(unobserved[0].cap).toBe(50);
		expect(unobserved[0].bypass).toBe(120);
		expect(unobserved[0].observed).toBe(false);
	});

	it('is empty for no windows and no config (the genuine hidden state)', () => {
		const { observed, unobserved } = mergeUsageWindows([], null);
		expect(observed).toHaveLength(0);
		expect(unobserved).toHaveLength(0);
	});
});

describe('editorWindowKeys', () => {
	it('always offers the two baseline windows, then observed, then configured', () => {
		const keys = editorWindowKeys([win('weekly_model:opus', 'Weekly Opus', 5)], {
			'weekly_model:extra': { cap_pct: 10 }
		});
		expect(keys.map((k) => k.key)).toEqual([
			'session',
			'weekly_all',
			'weekly_model:opus',
			'weekly_model:extra'
		]);
		expect(keys[2].label).toBe('opus');
	});
});

describe('dollar windows', () => {
	const usdWin = (key: string, amount: number): UsageWindow => ({
		key,
		kind: 'usd',
		label: windowLabelFromKey(key),
		utilization: 0,
		amount_usd: amount
	});

	it('labels and flags the dollar keys', () => {
		expect(windowLabelFromKey('session_usd')).toBe('Session');
		expect(windowLabelFromKey('usd_5h')).toBe('5h');
		expect(windowLabelFromKey('usd_7d')).toBe('7d');
		expect(isUsdKey('usd_7d')).toBe(true);
		expect(isUsdKey('weekly_all')).toBe(false);
	});

	it('merges spend with its dollar cap', () => {
		const { observed } = mergeUsageWindows([usdWin('usd_5h', 1.25), usdWin('usd_7d', 12)], {
			usd_5h: { cap_usd: 2, bypass_minutes: 15 }
		});
		expect(observed.map((r) => r.key)).toEqual(['usd_5h', 'usd_7d']);
		expect(observed[0].usd).toBe(true);
		expect(observed[0].amountUsd).toBe(1.25);
		expect(observed[0].capUsd).toBe(2);
		expect(observed[0].bypass).toBe(15);
		expect(observed[1].capUsd).toBeNull();
	});

	it('keeps a configured-but-unobserved dollar cap editable', () => {
		const { unobserved } = mergeUsageWindows([], { session_usd: { cap_usd: 5 } });
		expect(unobserved.map((r) => r.key)).toEqual(['session_usd']);
		expect(unobserved[0].usd).toBe(true);
		expect(unobserved[0].capUsd).toBe(5);
	});

	it('offers the dollar windows to a fireworks editor instead of the percent ones', () => {
		const keys = editorWindowKeys([], null, 'fireworks').map((k) => k.key);
		expect(keys).toEqual(['session_usd', 'usd_5h', 'usd_7d']);
		expect(editorWindowKeys([], null, 'anthropic').map((k) => k.key)).toEqual([
			'session',
			'weekly_all'
		]);
	});
});
