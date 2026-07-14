import { describe, expect, it } from 'vitest';
import type { UsageWindow } from '$lib/queries';
import { editorWindowKeys, mergeUsageWindows, windowLabelFromKey } from './usage-windows';

const win = (key: string, label: string, utilization: number, extra: Partial<UsageWindow> = {}): UsageWindow => ({
	key,
	kind: key === 'session' ? 'session' : key.startsWith('weekly_model:') ? 'weekly_scoped' : 'weekly_all',
	label,
	utilization,
	...extra
});

describe('windowLabelFromKey', () => {
	it('labels the canonical keys and falls back for model-scoped', () => {
		expect(windowLabelFromKey('session')).toBe('5h');
		expect(windowLabelFromKey('weekly_all')).toBe('Weekly (all models)');
		expect(windowLabelFromKey('weekly_model:fable')).toBe('Weekly fable');
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
		expect(observed[2].label).toBe('Weekly Fable');
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
		expect(keys[2].label).toBe('Weekly Opus');
	});
});
