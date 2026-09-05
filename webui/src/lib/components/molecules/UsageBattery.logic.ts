import type { AccountUsageEntry, UsagePace, UsageWindow } from '$lib/queries';

/** Fill colour of a battery bar, by headroom left in the window. */
export type HeadroomTone = 'ok' | 'warn' | 'danger' | 'unknown';

/** Pace glyph of a window: under the linear pace, on it, or burning past it. */
export type PaceState = 'leaf' | 'neutral' | 'flame';

const LEAF_BELOW = 0.8;
const FLAME_ABOVE = 1.2;

/** Green while more than half the window is left, amber down to 20%, then red. */
export function headroomTone(utilization: number | null): HeadroomTone {
	if (utilization === null || !Number.isFinite(utilization)) return 'unknown';
	const headroom = 100 - utilization;
	if (headroom > 50) return 'ok';
	if (headroom > 20) return 'warn';
	return 'danger';
}

export function paceState(pace: UsagePace | null | undefined): PaceState | null {
	if (!pace || !Number.isFinite(pace.ratio)) return null;
	if (pace.ratio < LEAF_BELOW) return 'leaf';
	if (pace.ratio > FLAME_ABOVE) return 'flame';
	return 'neutral';
}

/** Ms until the projected wall when it lands before the window resets — the
 *  "you will hit the limit before it refills" case; null otherwise. */
export function wallInMs(
	pace: UsagePace | null | undefined,
	resets: string | null | undefined,
	now: number
): number | null {
	if (!pace?.projected_wall_at || !resets) return null;
	const wall = Date.parse(pace.projected_wall_at);
	const reset = Date.parse(resets);
	if (!Number.isFinite(wall) || !Number.isFinite(reset) || wall >= reset) return null;
	return Math.max(0, wall - now);
}

/** Compact countdown: `38 min`, `2h10`, `3d 4h`. */
export function countdown(ms: number): string {
	const mins = Math.max(0, Math.round(ms / 60_000));
	if (mins < 60) return `${mins} min`;
	const hours = Math.floor(mins / 60);
	if (hours < 24) return `${hours}h${String(mins % 60).padStart(2, '0')}`;
	const days = Math.floor(hours / 24);
	return `${days}d ${hours % 24}h`;
}

/** The two bars of a battery, picked off a provider's windows. Dollar windows
 *  carry no quota share, so they read as unknown. */
export interface BatteryBars {
	fiveHour: UsageWindow | null;
	weekly: UsageWindow | null;
}

export function batteryBars(windows: UsageWindow[]): BatteryBars {
	const pick = (key: string) => windows.find((w) => w.key === key) ?? null;
	return { fiveHour: pick('session'), weekly: pick('weekly_all') };
}

export function barPct(w: UsageWindow | null): number | null {
	if (!w || !Number.isFinite(w.utilization)) return null;
	return Math.max(0, Math.min(100, Math.round(w.utilization)));
}

/** The window whose pace is worst (highest ratio), for the battery's glyph. */
export function worstPace(windows: (UsageWindow | null)[]): UsageWindow | null {
	let worst: UsageWindow | null = null;
	for (const w of windows) {
		if (!w?.pace) continue;
		if (!worst?.pace || w.pace.ratio > worst.pace.ratio) worst = w;
	}
	return worst;
}

/** One battery per provider that reported at least one window, in account
 *  order, so the header can render and group them. */
export interface BatteryEntry {
	providerId: string;
	provider: string;
	account: string;
	accountName: string;
	bars: BatteryBars;
	windows: UsageWindow[];
}

export function batteryEntries(rows: AccountUsageEntry[] | null | undefined): BatteryEntry[] {
	return (rows ?? [])
		.filter((r) => r.windows.length > 0)
		.map((r) => ({
			providerId: r.account_id,
			provider: r.provider,
			account: r.account,
			accountName: r.account_name,
			bars: batteryBars(r.windows),
			windows: r.windows
		}));
}

/** The single battery shown on narrow screens: for each bar the fullest
 *  window across all providers, so the worst headroom is what shows. */
export function aggregateBars(entries: BatteryEntry[]): BatteryBars {
	const fullest = (pick: (b: BatteryBars) => UsageWindow | null): UsageWindow | null => {
		let best: UsageWindow | null = null;
		for (const e of entries) {
			const w = pick(e.bars);
			if (w && (!best || w.utilization > best.utilization)) best = w;
		}
		return best;
	};
	return { fiveHour: fullest((b) => b.fiveHour), weekly: fullest((b) => b.weekly) };
}
