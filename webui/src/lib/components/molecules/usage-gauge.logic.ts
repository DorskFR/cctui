import type { UsageWindow } from '#lib/queries/index.ts';
import { barPct, type BatteryEntry } from './usage-battery.logic.ts';

export type GaugeWindow = 'session' | 'weekly_all';
export type GaugeTone = 'ok' | 'warn' | 'danger';

export const DEFAULT_SOFT_PCT = 80;

export interface GaugeReading {
	window: GaugeWindow;
	pct: number;
	tone: GaugeTone;
	resetsAt: string | null;
}

export type CapOf = (providerId: string, window: GaugeWindow) => number | null;

const TONE_RANK: Record<GaugeTone, number> = { ok: 0, warn: 1, danger: 2 };

export function gaugeTone(pct: number, cap: number | null): GaugeTone {
	if (pct >= 100) return 'danger';
	if (pct >= (cap ?? DEFAULT_SOFT_PCT)) return 'warn';
	return 'ok';
}

function reading(w: UsageWindow | null, window: GaugeWindow, cap: number | null): GaugeReading | null {
	const pct = barPct(w);
	if (w === null || pct === null) return null;
	return { window, pct, tone: gaugeTone(pct, cap), resetsAt: w.resets_at ?? null };
}

export function entryReadings(e: BatteryEntry, capOf: CapOf): GaugeReading[] {
	return [
		reading(e.bars.fiveHour, 'session', capOf(e.providerId, 'session')),
		reading(e.bars.weekly, 'weekly_all', capOf(e.providerId, 'weekly_all'))
	].filter((r): r is GaugeReading => r !== null);
}

/** Most severe tone first, then the fullest window. */
export function worstReading(readings: GaugeReading[]): GaugeReading | null {
	let worst: GaugeReading | null = null;
	for (const r of readings) {
		if (
			!worst ||
			TONE_RANK[r.tone] > TONE_RANK[worst.tone] ||
			(r.tone === worst.tone && r.pct > worst.pct)
		)
			worst = r;
	}
	return worst;
}
