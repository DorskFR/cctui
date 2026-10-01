// Spend readings shared with the TUI (`cctui-clientcore::spend`): what the
// windows cost, which models the dollars went to, and the daily token series a
// sparkline draws. No endpoint reports dollars per window, so per-model dollars
// are attributed as the Overview's cost tile does: a session's lifetime cost,
// booked to the window it was registered in. Times are unix milliseconds.
import { modelFamily } from '$lib/format';

export interface SessionSpend {
	/** `null` when the harness never reported one; grouped as `unknown`. */
	model: string | null;
	/** `null` for a session with no registration time: it counts in no window. */
	registeredAtMs: number | null;
	costUsd: number;
	tokens: number;
}

/** Window cut-offs — registered at or after a cut-off counts in that window. */
export interface Windows {
	todayMs: number;
	weekMs: number;
	monthMs: number;
}

export interface ModelSpendRow {
	/** Family, not the full model id. */
	model: string;
	today: number;
	week: number;
	month: number;
}

/** Lifetime cost of the sessions registered at or after `cutoffMs`. */
export function spendSince(sessions: readonly SessionSpend[], cutoffMs: number): number {
	let total = 0;
	for (const s of sessions) {
		if (s.registeredAtMs !== null && s.registeredAtMs >= cutoffMs) total += s.costUsd;
	}
	return total;
}

/** Dollars per model family per window, biggest 30-day spender first. Rows that
 *  cost nothing in any window are dropped. */
export function modelSpend(
	sessions: readonly SessionSpend[],
	windows: Windows,
): ModelSpendRow[] {
	const rows: ModelSpendRow[] = [];
	for (const s of sessions) {
		const at = s.registeredAtMs;
		if (at === null || at < windows.monthMs) continue;
		const name = s.model?.trim() ? modelFamily(s.model.trim()) : 'unknown';
		let row = rows.find((r) => r.model === name);
		if (!row) {
			row = { model: name, today: 0, week: 0, month: 0 };
			rows.push(row);
		}
		row.month += s.costUsd;
		if (at >= windows.weekMs) row.week += s.costUsd;
		if (at >= windows.todayMs) row.today += s.costUsd;
	}
	return rows
		.filter((r) => r.month > 0 || r.week > 0 || r.today > 0)
		.sort((a, b) => b.month - a.month || a.model.localeCompare(b.model));
}

export function spendTotals(rows: readonly ModelSpendRow[]): ModelSpendRow {
	return {
		model: 'total',
		today: rows.reduce((n, r) => n + r.today, 0),
		week: rows.reduce((n, r) => n + r.week, 0),
		month: rows.reduce((n, r) => n + r.month, 0),
	};
}

export interface DailyPoint {
	/** The local-midnight instant the caller truncated the bucket to. */
	dayMs: number;
	tokens: number;
}

const DAY_MS = 86_400_000;

/** Zero-fill into a dense oldest→newest series of `days` entries ending at
 *  `endDayMs`, so a sparkline has no gaps. `fillBuckets` at day granularity. */
export function fillDaily(
	points: readonly DailyPoint[],
	days: number,
	endDayMs: number,
): number[] {
	const out: number[] = [];
	for (let back = days - 1; back >= 0; back--) {
		const ms = endDayMs - back * DAY_MS;
		out.push(points.find((p) => p.dayMs === ms)?.tokens ?? 0);
	}
	return out;
}

/** The Langfuse cost as the chip writes it: cents over a dollar, mills under.
 *  Deliberately not `usd()` — most sessions sit under a dollar, where the third
 *  digit is the whole figure. */
export function langfuseCostLabel(costUsd: number): string {
	const cost = Number.isFinite(costUsd) ? costUsd : 0;
	return `$${cost.toFixed(cost >= 1 ? 2 : 3)}`;
}

/** A session with no traces yet gets no cost line, on either client. */
export function hasLangfuseCost(usage: { trace_count: number } | null | undefined): boolean {
	return !!usage && usage.trace_count > 0;
}
