export type StatTone = 'ok' | 'warn' | 'danger' | 'unknown';

export const THRESHOLDS = {
	quota: { warn: 50, danger: 80 },
	resource: { warn: 70, danger: 90 },
	/** Share of a quota window left unused when it reset. */
	loss: { warn: 20, danger: 50 }
} as const;

export type MetricKind = keyof typeof THRESHOLDS;

export function statTone(kind: MetricKind, pct: number | null | undefined): StatTone {
	if (pct === null || pct === undefined || !Number.isFinite(pct)) return 'unknown';
	const t = THRESHOLDS[kind];
	if (pct >= t.danger) return 'danger';
	if (pct >= t.warn) return 'warn';
	return 'ok';
}

const RANK: Record<StatTone, number> = { unknown: 0, ok: 1, warn: 2, danger: 3 };

export function worstTone(tones: readonly StatTone[]): StatTone {
	return tones.reduce<StatTone>((a, b) => (RANK[b] > RANK[a] ? b : a), 'unknown');
}
