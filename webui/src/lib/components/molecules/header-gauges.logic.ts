import type { AccountUsageEntry, UsagePace, UsageWindow } from '$lib/queries';

/** Pace glyph of a window: under the linear pace, on it, or burning past it. */
export type PaceState = 'leaf' | 'neutral' | 'flame';

const LEAF_BELOW = 0.8;
const FLAME_ABOVE = 1.2;
/** "Under half the sustainable pace" — the header gauge's 🍃 threshold. */
const HALF_PACE = 0.5;

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

/** The gauge's corner glyph: 🔥 once the projected wall lands before the reset,
 *  🍃 while under half the sustainable pace, nothing while on pace. */
export function gaugePace(w: UsageWindow | null, now: number): 'flame' | 'leaf' | null {
	if (!w?.pace || !Number.isFinite(w.pace.ratio)) return null;
	if (wallInMs(w.pace, w.resets_at, now) !== null) return 'flame';
	return w.pace.ratio < HALF_PACE ? 'leaf' : null;
}

/** The window a gauge stands for: the 5h session window, else the fullest one
 *  reported — a provider without a 5h window still gets its worst news shown. */
export function gaugeWindow(windows: UsageWindow[]): UsageWindow | null {
	const session = windows.find((w) => w.key === 'session');
	if (session) return session;
	let worst: UsageWindow | null = null;
	for (const w of windows) {
		if (!Number.isFinite(w.utilization)) continue;
		if (!worst || w.utilization > worst.utilization) worst = w;
	}
	return worst;
}

export function utilizationPct(w: UsageWindow | null): number | null {
	if (!w || !Number.isFinite(w.utilization)) return null;
	return Math.max(0, Math.min(100, Math.round(w.utilization)));
}

export interface GaugeEntry {
	providerId: string;
	provider: string;
	account: string;
	accountName: string;
	accountEmoji: string | null;
	window: UsageWindow | null;
	windows: UsageWindow[];
}

/** A gauge per pinned provider with usage, in the server's account order. */
export function gaugeEntries(rows: AccountUsageEntry[] | null | undefined): GaugeEntry[] {
	return (rows ?? [])
		.filter((r) => r.header_pin && r.windows.length > 0)
		.map((r) => ({
			providerId: r.account_id,
			provider: r.provider,
			account: r.account,
			accountName: r.account_name,
			accountEmoji: r.account_emoji,
			window: gaugeWindow(r.windows),
			windows: r.windows
		}));
}

/** Cells of one account, so the header can print the avatar once per group. */
export interface GaugeGroup {
	account: string;
	accountName: string;
	accountEmoji: string | null;
	entries: GaugeEntry[];
}

export function gaugeGroups(entries: GaugeEntry[]): GaugeGroup[] {
	const byAccount = new Map<string, GaugeGroup>();
	for (const e of entries) {
		const group = byAccount.get(e.account) ?? {
			account: e.account,
			accountName: e.accountName,
			accountEmoji: e.accountEmoji,
			entries: []
		};
		group.entries.push(e);
		byAccount.set(e.account, group);
	}
	return [...byAccount.values()];
}

/** The single cell narrow screens keep: the most-used provider of all. */
export function busiest(entries: GaugeEntry[]): GaugeEntry | null {
	let best: GaugeEntry | null = null;
	for (const e of entries) {
		const pct = utilizationPct(e.window);
		if (pct === null) continue;
		if (best === null || pct > (utilizationPct(best.window) ?? -1)) best = e;
	}
	return best ?? entries[0] ?? null;
}
