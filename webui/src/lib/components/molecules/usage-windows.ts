import type { SoftLimitConfig, UsageWindow } from '$lib/queries';

/** One merged row for the usage view: an observed window (or a configured-but-
 *  unobserved key with `utilization === null`) plus its soft-limit config. */
export interface UsageRow {
	key: string;
	label: string;
	/** null ⇒ configured but not present in the latest usage response. */
	utilization: number | null;
	/** USD spent in a dollar window; null for a percent window. */
	amountUsd: number | null;
	resets: string | null;
	cap: number | null;
	/** Dollar cap for a dollar window. */
	capUsd: number | null;
	bypass: number | null;
	observed: boolean;
	/** Dollar-denominated window: rendered and edited in $, not %. */
	usd: boolean;
}

const WEEKLY_MODEL_PREFIX = 'weekly_model:';

/** The dollar windows, in display order. Offered by the editor for
 *  pay-per-token providers, whose budgets are money, not a subscription share. */
export const USD_WINDOW_KEYS = ['session_usd', 'usd_5h', 'usd_7d'];

export function isUsdKey(key: string): boolean {
	return USD_WINDOW_KEYS.includes(key);
}

/** Human label for a canonical window key when the server didn't supply one
 *  (configured-but-unobserved keys carry no server label). */
export function windowLabelFromKey(key: string): string {
	if (key === 'session_usd') return 'Session spend';
	if (key === 'usd_5h') return '5h spend';
	if (key === 'usd_7d') return '7d spend';
	if (key === 'session') return '5h';
	if (key === 'weekly_all') return 'Weekly (all models)';
	if (key.startsWith(WEEKLY_MODEL_PREFIX)) return `Weekly ${key.slice(WEEKLY_MODEL_PREFIX.length)}`;
	return key;
}

/** Merge the observed usage windows with the configured soft-limit map. Observed
 *  windows keep server order and come first; configured keys with no matching
 *  window follow as `unobserved` so they stay visible/editable. */
export function mergeUsageWindows(
	windows: UsageWindow[],
	softLimits: Record<string, SoftLimitConfig> | null | undefined
): { observed: UsageRow[]; unobserved: UsageRow[] } {
	const limits = softLimits ?? {};
	const seen = new Set<string>();
	const observed = windows.map((w): UsageRow => {
		seen.add(w.key);
		const l = limits[w.key];
		return {
			key: w.key,
			label: w.label || windowLabelFromKey(w.key),
			utilization: w.utilization,
			amountUsd: w.amount_usd ?? null,
			resets: w.resets_at ?? null,
			cap: l?.cap_pct ?? null,
			capUsd: l?.cap_usd ?? null,
			bypass: l?.bypass_minutes ?? null,
			observed: true,
			usd: isUsdKey(w.key)
		};
	});
	const unobserved = Object.keys(limits)
		.filter((k) => !seen.has(k))
		.map((k): UsageRow => {
			const l = limits[k];
			return {
				key: k,
				label: windowLabelFromKey(k),
				utilization: null,
				amountUsd: null,
				resets: null,
				cap: l?.cap_pct ?? null,
				capUsd: l?.cap_usd ?? null,
				bypass: l?.bypass_minutes ?? null,
				observed: false,
				usd: isUsdKey(k)
			};
		});
	return { observed, unobserved };
}

/** Keys to offer in the account editor: the two non-model canonical windows are
 *  always available (so caps can be set before any usage is reported), then any
 *  observed window keys, then any configured keys — deduped, order preserved. */
export function editorWindowKeys(
	windows: UsageWindow[],
	softLimits: Record<string, SoftLimitConfig> | null | undefined,
	family?: string | null
): { key: string; label: string }[] {
	const order = family === 'fireworks' ? [...USD_WINDOW_KEYS] : ['session', 'weekly_all'];
	const labels: Record<string, string> = {};
	for (const w of windows) {
		if (!order.includes(w.key)) order.push(w.key);
		labels[w.key] = w.label || windowLabelFromKey(w.key);
	}
	for (const k of Object.keys(softLimits ?? {})) if (!order.includes(k)) order.push(k);
	return order.map((k) => ({ key: k, label: labels[k] ?? windowLabelFromKey(k) }));
}
