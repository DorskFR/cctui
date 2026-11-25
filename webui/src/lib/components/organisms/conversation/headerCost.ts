import type { LangfuseSessionUsage } from '@bindings/LangfuseSessionUsage';

export type HeaderCost =
	| { source: 'langfuse' | 'harness'; usd: number }
	| { source: 'plan' | 'none' };

/** The one cost a session shows: Langfuse once it has traces, else the
 *  harness's own figure, else — tokens spent at $0 — billed to the plan. */
export function headerCost(
	harnessUsd: number,
	totalTokens: number,
	langfuse: Pick<LangfuseSessionUsage, 'cost_usd' | 'trace_count'> | null | undefined
): HeaderCost {
	if (langfuse && langfuse.trace_count > 0) return { source: 'langfuse', usd: Number(langfuse.cost_usd) || 0 };
	if (harnessUsd > 0) return { source: 'harness', usd: harnessUsd };
	if (totalTokens > 0) return { source: 'plan' };
	return { source: 'none' };
}
