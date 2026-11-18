import type { StorageLifecycleRules } from '@bindings/StorageLifecycleRules';
import type { StorageSessionSize } from '@bindings/StorageSessionSize';
import { m } from '#lib/paraglide/messages.js';

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'];

export function fmtBytes(n: number): string {
	let v = Math.max(0, n);
	let i = 0;
	while (v >= 1024 && i < UNITS.length - 1) {
		v /= 1024;
		i++;
	}
	return i === 0 ? `${v} B` : `${v.toFixed(v < 10 ? 1 : 0)} ${UNITS[i]}`;
}

export type LifecycleDraft = {
	trimOn: boolean;
	trimDays: number;
	trimKb: number;
	dropOn: boolean;
	dropDays: number;
};

export const MAX_DAYS = 3650;
export const MAX_TRIM_KB = 64 * 1024;
const DEFAULT_DRAFT: LifecycleDraft = { trimOn: false, trimDays: 30, trimKb: 16, dropOn: false, dropDays: 180 };

export function draftFrom(rules: StorageLifecycleRules): LifecycleDraft {
	const { trim, drop } = rules;
	return {
		trimOn: trim?.enabled ?? false,
		trimDays: trim?.after_days ?? DEFAULT_DRAFT.trimDays,
		trimKb: trim ? Math.round(trim.above_bytes / 1024) : DEFAULT_DRAFT.trimKb,
		dropOn: drop?.enabled ?? false,
		dropDays: drop?.after_days ?? DEFAULT_DRAFT.dropDays
	};
}

export function rulesFrom(d: LifecycleDraft): StorageLifecycleRules {
	return {
		trim: d.trimOn
			? { enabled: true, after_days: Math.trunc(d.trimDays), above_bytes: Math.trunc(d.trimKb) * 1024 }
			: null,
		drop: d.dropOn ? { enabled: true, after_days: Math.trunc(d.dropDays) } : null
	};
}

const inRange = (v: number, max: number) => Number.isInteger(v) && v >= 1 && v <= max;

export function draftValid(d: LifecycleDraft): boolean {
	return (
		(!d.trimOn || (inRange(d.trimDays, MAX_DAYS) && inRange(d.trimKb, MAX_TRIM_KB))) &&
		(!d.dropOn || inRange(d.dropDays, MAX_DAYS))
	);
}

/** Enabled rules only, so a stored-but-disabled rule equals an absent one. */
export function rulesKey(rules: StorageLifecycleRules): string {
	const trim = rules.trim?.enabled ? `trim:${rules.trim.after_days}:${rules.trim.above_bytes}` : '';
	const drop = rules.drop?.enabled ? `drop:${rules.drop.after_days}` : '';
	return `${trim}|${drop}`;
}

export const enablesAnything = (rules: StorageLifecycleRules) => rulesKey(rules) !== '|';

export function canSave(draft: LifecycleDraft, saved: StorageLifecycleRules, previewed: string | null): boolean {
	if (!draftValid(draft)) return false;
	const rules = rulesFrom(draft);
	const key = rulesKey(rules);
	if (key === rulesKey(saved)) return false;
	return !enablesAnything(rules) || previewed === key;
}

export const isExact = (percent: number) => percent >= 100;

const BANDS: Record<string, () => string> = {
	lt_1k: m.settings_server_storage_band_lt_1k,
	k1_10: m.settings_server_storage_band_k1_10,
	k10_100: m.settings_server_storage_band_k10_100,
	k100_1m: m.settings_server_storage_band_k100_1m,
	ge_1m: m.settings_server_storage_band_ge_1m,
	lt_1d: m.settings_server_storage_band_lt_1d,
	d1_7: m.settings_server_storage_band_d1_7,
	d7_30: m.settings_server_storage_band_d7_30,
	d30_90: m.settings_server_storage_band_d30_90,
	ge_90d: m.settings_server_storage_band_ge_90d
};

export const bandLabel = (key: string) => BANDS[key]?.() ?? key;

export const sessionLabel = (s: StorageSessionSize) => s.name?.trim() || s.session_id.slice(0, 8);
