import { describe, expect, it } from 'vitest';
import { statTone, THRESHOLDS, worstTone } from './tone';

describe('statTone', () => {
	it('turns warn and danger at the named thresholds of each metric', () => {
		for (const kind of ['quota', 'resource', 'loss'] as const) {
			const t = THRESHOLDS[kind];
			expect(statTone(kind, t.warn - 0.1)).toBe('ok');
			expect(statTone(kind, t.warn)).toBe('warn');
			expect(statTone(kind, t.danger - 0.1)).toBe('warn');
			expect(statTone(kind, t.danger)).toBe('danger');
		}
	});

	it('reads a missing value as unknown, never as zero', () => {
		expect(statTone('quota', null)).toBe('unknown');
		expect(statTone('resource', undefined)).toBe('unknown');
		expect(statTone('quota', Number.NaN)).toBe('unknown');
	});
});

describe('worstTone', () => {
	it('picks the most severe reading', () => {
		expect(worstTone(['ok', 'danger', 'warn'])).toBe('danger');
		expect(worstTone(['unknown', 'ok'])).toBe('ok');
		expect(worstTone([])).toBe('unknown');
	});
});
