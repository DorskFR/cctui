export type CostSource = 'langfuse' | 'harness';

export interface HeaderCost {
	value: number;
	source: CostSource;
}

/**
 * The one cost the header shows. Langfuse is the billed figure when it has
 * traces; the harness figure is an estimate, and a zero estimate means the
 * tokens were billed to a subscription plan, so it shows nothing.
 */
export function headerCost(
	harnessUsd: number,
	langfuse: { cost_usd: number | string; trace_count: number } | null | undefined
): HeaderCost | null {
	if (langfuse && langfuse.trace_count > 0) {
		const v = Number(langfuse.cost_usd);
		return { value: Number.isFinite(v) ? v : 0, source: 'langfuse' };
	}
	const h = Number(harnessUsd);
	return Number.isFinite(h) && h > 0 ? { value: h, source: 'harness' } : null;
}

export function showHarnessEstimate(cost: HeaderCost | null, harnessUsd: number): boolean {
	return cost?.source === 'langfuse' && Number(harnessUsd) > 0;
}
