import type {
	AccountModel,
	OAuthAccount,
	RateLimits,
	SoftLimitConfig,
	UsageNotices
} from '$lib/queries';
import { isUsdKey } from '$lib/components/molecules/usage-windows';
import { PROVIDER_KINDS, providerFamily, type ProviderKind } from '$lib/providers';

export type SoftEdit = { cap: number | null; capUsd: number | null; bypass: number | null };

/** Empty ⇒ null, else a clamped non-negative integer; tolerates the number a
 *  number-input binds or a stray string. */
function softNum(v: number | string | null | undefined): number | null {
	if (v === null || v === undefined || v === '') return null;
	const n = Math.round(Number(v));
	return Number.isFinite(n) ? Math.max(0, n) : null;
}

function softUsd(v: number | string | null | undefined): number | null {
	if (v === null || v === undefined || v === '') return null;
	const n = Number(v);
	return Number.isFinite(n) ? Math.max(0, n) : null;
}

/** The whole replacement map: windows with neither cap nor bypass are dropped. */
export function buildSoftLimits(edits: Record<string, SoftEdit>): Record<string, SoftLimitConfig> {
	const out: Record<string, SoftLimitConfig> = {};
	for (const [key, v] of Object.entries(edits)) {
		const bypass = softNum(v.bypass);
		if (isUsdKey(key)) {
			const capUsd = softUsd(v.capUsd);
			if (capUsd !== null || bypass !== null) out[key] = { cap_usd: capUsd, bypass_minutes: bypass };
			continue;
		}
		const cap = softNum(v.cap);
		if (cap !== null || bypass !== null) out[key] = { cap_pct: cap, bypass_minutes: bypass };
	}
	return out;
}

export function buildRateLimits(edits: { rpm: number | null; tpm: number | null }): RateLimits {
	return { rpm: softNum(edits.rpm), tpm: softNum(edits.tpm) };
}

export function buildUsageNotices(edit: UsageNotices): UsageNotices {
	const step = Math.round(Number(edit.step_pct));
	return {
		enabled: edit.enabled,
		step_pct: Number.isFinite(step) && step >= 1 && step <= 100 ? step : 10
	};
}

export function aliasObject(rows: { alias: string; model: string }[]): Record<string, string> {
	const out: Record<string, string> = {};
	for (const r of rows) {
		const a = r.alias.trim();
		const mo = r.model.trim();
		if (a && mo) out[a] = mo;
	}
	return out;
}

export function envObject(rows: { name: string; value: string }[]): Record<string, string> {
	const out: Record<string, string> = {};
	for (const r of rows) {
		const n = r.name.trim();
		if (n) out[n] = r.value;
	}
	return out;
}

export function fwModelList(models: AccountModel[]): AccountModel[] {
	return models
		.map((r) => ({ ...r, model: r.model.trim(), label: r.label.trim() || r.model.trim() }))
		.filter((r) => r.model);
}

export function modelList(rows: { model: string; label: string }[]): AccountModel[] {
	return rows
		.map((r) => ({ model: r.model.trim(), label: r.label.trim() || r.model.trim() }))
		.filter((r) => r.model);
}

/** Provider kinds an account can still add: one per family, like the server's unique index. */
export function availableKinds(a: OAuthAccount): ProviderKind[] {
	const taken = new Set(a.providers.map((p) => p.family));
	return PROVIDER_KINDS.map((k) => k.value).filter((v) => !taken.has(providerFamily(v)));
}
