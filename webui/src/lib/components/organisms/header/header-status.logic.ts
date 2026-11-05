import type { AccountUsageEntry, UsageWindow } from '$lib/queries';
import type { SoftLimitConfig } from '@bindings/SoftLimitConfig';
import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
import { isStale, worstTone } from '$lib/components/molecules/resource-gauge.logic';

export type StatusTone = 'ok' | 'warn' | 'danger';
export type HealthTone = StatusTone | 'unknown';

export type SoftCaps = Record<string, SoftLimitConfig> | null | undefined;

export const DEFAULT_WARN_PCT = 90;

export interface WorstUsage {
	short: string;
	pct: number;
	tone: StatusTone;
	accountName: string;
	provider: string;
}

export function windowShort(w: Pick<UsageWindow, 'key' | 'kind' | 'label'>): string {
	if (w.key === 'session' || w.kind === 'session') return '5h';
	if (w.key.startsWith('weekly') || w.kind.startsWith('weekly')) return '7d';
	return w.label;
}

export function usageTone(pct: number, cap: number | null | undefined): StatusTone {
	if (pct >= 100) return 'danger';
	if (pct >= (cap ?? DEFAULT_WARN_PCT)) return 'warn';
	return 'ok';
}

const TONE_RANK: Record<HealthTone, number> = { ok: 0, unknown: 1, warn: 2, danger: 3 };

const isPercentWindow = (w: UsageWindow) =>
	Number.isFinite(w.utilization) && (w.amount_usd === null || w.amount_usd === undefined);

/** Worst tone first, then fullest. */
export function worstUsage(
	rows: AccountUsageEntry[] | null | undefined,
	capsFor: (providerId: string) => SoftCaps
): WorstUsage | null {
	let worst: WorstUsage | null = null;
	for (const r of rows ?? []) {
		if (!r.header_pin) continue;
		const caps = capsFor(r.account_id);
		for (const w of r.windows) {
			if (!isPercentWindow(w)) continue;
			const pct = Math.max(0, Math.round(w.utilization));
			const tone = usageTone(pct, caps?.[w.key]?.cap_pct);
			const better =
				!worst ||
				TONE_RANK[tone] > TONE_RANK[worst.tone] ||
				(tone === worst.tone && pct > worst.pct);
			if (better) {
				worst = { short: windowShort(w), pct, tone, accountName: r.account_name, provider: r.provider };
			}
		}
	}
	return worst;
}

export function machineHealth(rows: MachineResourcesRow[], now: number): HealthTone | null {
	if (rows.length === 0) return null;
	let worst: HealthTone = 'ok';
	for (const r of rows) {
		const tone: HealthTone =
			!r.resources || isStale(r.updated_at, now) ? 'unknown' : worstTone(r.resources);
		if (TONE_RANK[tone] > TONE_RANK[worst]) worst = tone;
	}
	return worst;
}

export function worseTone(a: HealthTone | null, b: HealthTone | null): HealthTone | null {
	if (!a) return b;
	if (!b) return a;
	return TONE_RANK[b] > TONE_RANK[a] ? b : a;
}
