import type { SessionListItem } from '@bindings/SessionListItem';
import { compact } from '#lib/format.ts';

export type ContextFields = Pick<
	SessionListItem,
	'context_tokens' | 'context_window' | 'auto_followup_at'
>;

export interface ContextMeterModel {
	label: string;
	/** 0..1 of the window; null when the window is unknown. */
	fill: number | null;
	mark: number | null;
	followupAt: number | null;
}

export function contextTokens(n: number): string {
	return compact(n).replace(/\.0(?=[kMB]$)/, '');
}

const ratio = (n: number, of: number) => Math.min(1, Math.max(0, n / of));

export function contextMeter(s: ContextFields): ContextMeterModel | null {
	const used = s.context_tokens ?? null;
	if (used === null) return null;
	const window = s.context_window ?? null;
	const followupAt = s.auto_followup_at ?? null;
	if (window === null || window <= 0) {
		return { label: contextTokens(used), fill: null, mark: null, followupAt };
	}
	const pct = Math.round((used / window) * 100);
	return {
		label: `${contextTokens(used)} / ${contextTokens(window)} · ${pct}%`,
		fill: ratio(used, window),
		mark: followupAt === null ? null : ratio(followupAt, window),
		followupAt
	};
}

export function followupDue(s: ContextFields): boolean {
	const at = s.auto_followup_at ?? null;
	const used = s.context_tokens ?? null;
	return at !== null && used !== null && used >= at;
}
