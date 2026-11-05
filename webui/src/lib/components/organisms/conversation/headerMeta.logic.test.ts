import { describe, expect, it } from 'vitest';
import { headerCost, showHarnessEstimate } from './headerMeta.logic';

describe('headerCost', () => {
	it('prefers Langfuse when it reported traces, whatever the harness says', () => {
		expect(headerCost(0, { cost_usd: 2.38, trace_count: 50 })).toEqual({ value: 2.38, source: 'langfuse' });
		expect(headerCost(1.1, { cost_usd: '2.38', trace_count: 50 })).toEqual({ value: 2.38, source: 'langfuse' });
	});

	it('ignores a Langfuse answer without traces', () => {
		expect(headerCost(0.5, { cost_usd: 0, trace_count: 0 })).toEqual({ value: 0.5, source: 'harness' });
	});

	it('hides a zero harness cost: those tokens were billed to a plan', () => {
		expect(headerCost(0, null)).toBeNull();
		expect(headerCost(0, undefined)).toBeNull();
		expect(headerCost(Number.NaN, null)).toBeNull();
	});

	it('falls back to the harness estimate when there is no Langfuse', () => {
		expect(headerCost(3.2, null)).toEqual({ value: 3.2, source: 'harness' });
	});
});

describe('showHarnessEstimate', () => {
	it('adds the estimate row only beside a Langfuse cost and only when non-zero', () => {
		const lf = headerCost(1, { cost_usd: 2, trace_count: 3 });
		expect(showHarnessEstimate(lf, 1)).toBe(true);
		expect(showHarnessEstimate(lf, 0)).toBe(false);
		expect(showHarnessEstimate(headerCost(1, null), 1)).toBe(false);
		expect(showHarnessEstimate(null, 1)).toBe(false);
	});
});
