import { describe, expect, it } from 'vitest';
import { headerCost } from './headerCost';

describe('headerCost', () => {
	it('prefers Langfuse once it has traces', () => {
		expect(headerCost(0, 10, { cost_usd: 2.38, trace_count: 50 })).toEqual({ source: 'langfuse', usd: 2.38 });
		expect(headerCost(1.2, 10, { cost_usd: 2.38, trace_count: 50 })).toEqual({ source: 'langfuse', usd: 2.38 });
	});

	it('falls back to the harness cost when Langfuse has nothing', () => {
		expect(headerCost(0.41, 10, { cost_usd: 0, trace_count: 0 })).toEqual({ source: 'harness', usd: 0.41 });
		expect(headerCost(0.41, 10, null)).toEqual({ source: 'harness', usd: 0.41 });
	});

	it('reads tokens spent at $0 as billed to the plan, not as $0.00', () => {
		expect(headerCost(0, 25_000_000, null)).toEqual({ source: 'plan' });
	});

	it('shows nothing for an empty session', () => {
		expect(headerCost(0, 0, undefined)).toEqual({ source: 'none' });
	});
});
