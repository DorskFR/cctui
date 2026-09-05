import type { SoftLimitConfig } from '$lib/queries';
import { countdown } from './usage-battery.logic';

/** A cap parked at 100% is no cap: the stored config drops the window's `cap_pct`. */
export function capFromBar(cap: number): number | null {
	return cap >= 100 ? null : Math.max(0, Math.round(cap));
}

export function capToBar(cap: number | null | undefined): number {
	return cap == null || cap < 0 || cap > 100 ? 100 : cap;
}

export function usdPct(amountUsd: number | null, capUsd: number | null | undefined): number | null {
	if (amountUsd === null || capUsd == null || capUsd <= 0) return null;
	return Math.max(0, Math.min(100, Math.round((amountUsd / capUsd) * 100)));
}

export const money = (n: number) => `$${n.toFixed(2)}`;

export function resetIn(resets: string | null | undefined, now: number): string | null {
	const at = resets ? Date.parse(resets) : Number.NaN;
	if (!Number.isFinite(at) || at <= now) return null;
	return countdown(at - now);
}

export function usdReadout(
	amountUsd: number | null,
	capUsd: number | null | undefined
): string | null {
	if (amountUsd === null) return null;
	return capUsd == null ? money(amountUsd) : `${money(amountUsd)} / ${money(capUsd)}`;
}

/** The whole replacement map the PATCH expects; a window left with neither
 *  cap nor bypass is dropped rather than stored empty. */
export function withCap(
	limits: Record<string, SoftLimitConfig> | null | undefined,
	key: string,
	cap: number | null
): Record<string, SoftLimitConfig> {
	const out: Record<string, SoftLimitConfig> = { ...(limits ?? {}) };
	const next: SoftLimitConfig = { ...(out[key] ?? {}), cap_pct: cap };
	if (next.cap_pct == null && next.cap_usd == null && next.bypass_minutes == null) delete out[key];
	else out[key] = next;
	return out;
}
