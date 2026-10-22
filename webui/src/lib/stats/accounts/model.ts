import type { RowTone } from '@dorsk/tsumikit';
import type { AccountLedgerRow } from '@bindings/AccountLedgerRow';
import type { AccountLedgerTotal } from '@bindings/AccountLedgerTotal';
import type { AccountUsageEntry } from '@bindings/AccountUsageEntry';
import type { UsageWindowView } from '@bindings/UsageWindowView';
import { m } from '$lib/paraglide/messages';
import { statTone, type StatTone } from '../tone';

export const FIVE_HOUR = 'session';
export const WEEKLY = 'weekly_all';
export const LEDGER_LOOKBACK_MS = 7 * 24 * 3600_000;

/** Rounded down to the hour so every view mounted within the hour shares one
 *  ledger cache entry. */
export function ledgerFrom(nowMs: number): string {
	const from = nowMs - LEDGER_LOOKBACK_MS;
	return new Date(from - (from % 3600_000)).toISOString();
}

export type PushedUsage = Pick<
	AccountUsageEntry,
	'account_id' | 'provider' | 'usage' | 'windows' | 'age_secs' | 'limit_reset'
>;

/** Returns `old` itself when the credential is not cached, so an unrelated push
 *  cannot churn the query cache. */
export function patchUsageEntries(
	old: AccountUsageEntry[] | undefined,
	credentialId: string,
	usage: PushedUsage
): AccountUsageEntry[] | undefined {
	if (!old?.some((e) => e.account_id === credentialId)) return old;
	return old.map((e) =>
		e.account_id === credentialId ? { ...e, ...usage, account_id: e.account_id } : e
	);
}

export function usageOf(
	rows: readonly AccountUsageEntry[] | undefined,
	credentialId: string
): AccountUsageEntry | null {
	return rows?.find((r) => r.account_id === credentialId) ?? null;
}

export function windowLabel(key: string, fallback: string): string {
	if (key === FIVE_HOUR) return m.usage_battery_five_hour();
	if (key === WEEKLY) return m.usage_battery_weekly();
	return fallback;
}

export function credentialLabels(
	items: readonly { id: string; name: string; provider: string }[],
	providerName: (p: string) => string = (p) => p
): Map<string, string> {
	const providers = new Map<string, Set<string>>();
	for (const i of items) providers.set(i.name, (providers.get(i.name) ?? new Set()).add(i.id));
	return new Map(
		items.map((i) => [
			i.id,
			(providers.get(i.name)?.size ?? 0) > 1 ? `${i.name} · ${providerName(i.provider)}` : i.name
		])
	);
}

export interface CurrentWindow {
	pct: number | null;
	resetsAt: string | null;
}

export interface CurrentRow {
	id: string;
	label: string;
	emoji: string | null;
	provider: string;
	fiveHour: CurrentWindow | null;
	weekly: CurrentWindow | null;
}

function current(windows: readonly UsageWindowView[], key: string): CurrentWindow | null {
	const w = windows.find((x) => x.key === key);
	if (!w) return null;
	return {
		pct: Number.isFinite(w.utilization) ? w.utilization : null,
		resetsAt: w.resets_at ?? null
	};
}

export function currentRows(
	entries: readonly AccountUsageEntry[] | undefined,
	providerName?: (p: string) => string
): CurrentRow[] {
	const rows = (entries ?? []).filter((e) =>
		e.windows.some((w) => w.key === FIVE_HOUR || w.key === WEEKLY)
	);
	const labels = credentialLabels(
		rows.map((e) => ({ id: e.account_id, name: e.account_name, provider: e.provider })),
		providerName
	);
	return rows.map((e) => ({
		id: e.account_id,
		label: labels.get(e.account_id) ?? e.account_name,
		emoji: e.account_emoji,
		provider: e.provider,
		fiveHour: current(e.windows, FIVE_HOUR),
		weekly: current(e.windows, WEEKLY)
	}));
}

/** A sampling gap means the last reading predates the reset: the real loss is
 *  at most the figure shown. */
export function lostText(row: Pick<AccountLedgerRow, 'idle' | 'sampling_gap' | 'unused_pct'>): string {
	if (row.idle) return m.account_stats_idle();
	return `${row.sampling_gap ? '≤' : ''}${Math.round(row.unused_pct)}%`;
}

export function hoursText(hours: number | null): string | null {
	if (hours === null || !Number.isFinite(hours)) return null;
	return m.account_stats_hours({ hours: hours < 10 ? hours.toFixed(1) : String(Math.round(hours)) });
}

export function usdText(usd: number | null): string | null {
	return usd === null || !Number.isFinite(usd) ? null : `$${usd.toFixed(2)}`;
}

export function lossTone(row: Pick<AccountLedgerRow, 'idle' | 'unused_pct'>): StatTone {
	return row.idle ? 'unknown' : statTone('loss', row.unused_pct);
}

export function ledgerRowTone(row: Pick<AccountLedgerRow, 'idle' | 'unused_pct'>): RowTone | undefined {
	const t = lossTone(row);
	return t === 'unknown' ? undefined : t;
}

export interface LedgerFilter {
	account: string | null;
	window: string | null;
}

export function filterLedger(
	rows: readonly AccountLedgerRow[],
	f: LedgerFilter
): AccountLedgerRow[] {
	return rows.filter(
		(r) => (!f.account || r.account_id === f.account) && (!f.window || r.window_key === f.window)
	);
}

export type LedgerSortKey = 'resets_at' | 'account' | 'window' | 'used' | 'lost' | 'hours';

const SORT_VALUE: Record<LedgerSortKey, (r: AccountLedgerRow) => number | string | null> = {
	resets_at: (r) => Date.parse(r.resets_at),
	account: (r) => r.account_name.toLowerCase(),
	window: (r) => r.window_label.toLowerCase(),
	used: (r) => (r.idle ? null : r.final_utilization),
	lost: (r) => (r.idle ? null : r.unused_pct),
	hours: (r) => (r.idle ? null : r.unused_capacity_hours)
};

/** Idle resets sort last in either direction instead of passing for 100% lost. */
export function sortLedger(
	rows: readonly AccountLedgerRow[],
	key: LedgerSortKey,
	dir: 'asc' | 'desc'
): AccountLedgerRow[] {
	const value = SORT_VALUE[key];
	const sign = dir === 'asc' ? 1 : -1;
	return [...rows].sort((a, b) => {
		const va = value(a);
		const vb = value(b);
		if (va === null || vb === null) return va === vb ? 0 : va === null ? 1 : -1;
		return (va < vb ? -1 : va > vb ? 1 : 0) * sign;
	});
}

export interface Option {
	value: string;
	label: string;
}

export function ledgerOptions(
	rows: readonly AccountLedgerRow[],
	providerName?: (p: string) => string
): { accounts: Option[]; windows: Option[] } {
	const creds = new Map<string, { id: string; name: string; provider: string }>();
	const windows = new Map<string, string>();
	for (const r of rows) {
		creds.set(r.account_id, { id: r.account_id, name: r.account_name, provider: r.provider });
		windows.set(r.window_key, windowLabel(r.window_key, r.window_label));
	}
	const labels = credentialLabels([...creds.values()], providerName);
	const byLabel = (a: Option, b: Option) => a.label.localeCompare(b.label);
	return {
		accounts: [...labels].map(([value, label]) => ({ value, label })).sort(byLabel),
		windows: [...windows].map(([value, label]) => ({ value, label })).sort(byLabel)
	};
}

export function resetClock(iso: string, nowMs: number, locale: string, timeZone?: string): string {
	const at = new Date(iso);
	const day = (d: Date) => d.toLocaleDateString('en-CA', { timeZone });
	const time = at.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit', timeZone });
	if (day(at) === day(new Date(nowMs))) return time;
	return `${at.toLocaleDateString(locale, { weekday: 'short', timeZone })} ${time}`;
}

export interface OneLiner {
	id: string;
	emoji: string | null;
	text: string;
	tone: StatTone;
}

/** One line per credential: its latest reset that lost something, or "idle"
 *  when every reset in range went unused. Never summed across accounts. */
export function oneLiners(
	rows: readonly AccountLedgerRow[],
	clock: (iso: string) => string,
	providerName?: (p: string) => string
): OneLiner[] {
	const byCred = new Map<string, AccountLedgerRow[]>();
	for (const r of rows) byCred.set(r.account_id, [...(byCred.get(r.account_id) ?? []), r]);
	const labels = credentialLabels(
		[...byCred.values()].map((rs) => ({ id: rs[0].account_id, name: rs[0].account_name, provider: rs[0].provider })),
		providerName
	);
	const lines = [...byCred.entries()].map(([id, rs]) => {
		const account = labels.get(id) ?? rs[0].account_name;
		const latest = rs
			.filter((r) => !r.idle)
			.sort((a, b) => Date.parse(b.resets_at) - Date.parse(a.resets_at))[0];
		if (!latest) {
			return { id, emoji: rs[0].account_emoji, text: m.account_stats_line_idle({ account }), tone: 'unknown' as const };
		}
		return {
			id,
			emoji: latest.account_emoji,
			text: m.account_stats_line_lost({
				account,
				lost: lostText(latest),
				window: windowLabel(latest.window_key, latest.window_label),
				time: clock(latest.resets_at)
			}),
			tone: lossTone(latest)
		};
	});
	return lines.sort((a, b) => a.text.localeCompare(b.text));
}

export function totalText(t: AccountLedgerTotal, fallbackLabel = t.window_key): string {
	const window = windowLabel(t.window_key, fallbackLabel);
	if (t.closes > 0 && t.idle_closes >= t.closes) {
		return m.account_stats_total_idle({ window, resets: t.closes });
	}
	const lost = t.unused_pct_mean === null ? '–' : `${Math.round(t.unused_pct_mean)}%`;
	const head = m.account_stats_total({ window, resets: t.closes, lost, idle: t.idle_closes });
	const hours = hoursText(t.unused_capacity_hours);
	return hours ? `${head} · ${hours}` : head;
}

export interface TotalGroup {
	id: string;
	label: string;
	lines: string[];
}

export function totalGroups(
	totals: readonly AccountLedgerTotal[],
	rows: readonly AccountLedgerRow[],
	providerName?: (p: string) => string
): TotalGroup[] {
	const provider = new Map(rows.map((r) => [r.account_id, r.provider]));
	const windowNames = new Map(rows.map((r) => [r.window_key, r.window_label]));
	const labels = credentialLabels(
		totals.map((t) => ({ id: t.account_id, name: t.account_name, provider: provider.get(t.account_id) ?? '' })),
		providerName
	);
	const groups = new Map<string, TotalGroup>();
	for (const t of totals) {
		const g = groups.get(t.account_id) ?? {
			id: t.account_id,
			label: labels.get(t.account_id) ?? t.account_name,
			lines: []
		};
		g.lines.push(totalText(t, windowNames.get(t.window_key)));
		groups.set(t.account_id, g);
	}
	return [...groups.values()].sort((a, b) => a.label.localeCompare(b.label));
}
