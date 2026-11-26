import type { SoftLimitConfig } from '#lib/queries/index.ts';
import { countdown } from './usage-battery.logic';

/** A cap parked at 100% is no cap: the stored config drops the window's `cap_pct`. */
export function capFromBar(cap: number | null): number | null {
	if (cap === null) return null;
	return cap >= 100 ? null : Math.max(0, Math.round(cap));
}

export function capToBar(cap: number | null | undefined): number {
	return cap == null || cap < 0 || cap > 100 ? 100 : cap;
}

export function usdPct(amountUsd: number | null, capUsd: number | null | undefined): number | null {
	if (amountUsd === null || capUsd == null || capUsd <= 0) return null;
	return Math.max(0, Math.min(100, Math.round((amountUsd / capUsd) * 100)));
}

/** Shared column widths so every usage bar track lines up; overridable per list. */
export const USAGE_LABEL_W = 'var(--usage-label-w, 4rem)';
/** Until the readouts are measured: fits "$1234.56 / $2000.00" and "Not currently reported". */
export const USAGE_READOUT_W = 'var(--usage-readout-w, 8.75rem)';
const READOUT_FALLBACK_REM = 8.75;
/** Label column, both column gaps and the narrowest useful track. */
export const USAGE_ROW_FIXED_REM = 10;

/** Zero (unmeasured) width is never dense; an unmeasured readout counts as the fallback column. */
export const isDenseWidth = (width: number, readoutPx = 0, remPx = 16): boolean =>
	width > 0 && width < (readoutPx || READOUT_FALLBACK_REM * remPx) + USAGE_ROW_FIXED_REM * remPx;

export type ReadoutMeasure = { group: unknown; right: number; width: number };

/** Rows of one group whose readouts end at the same x (±1px) share the widest readout. */
export function readoutColumns(rows: ReadoutMeasure[]): number[] {
	const stacks: { group: unknown; right: number; width: number }[] = [];
	const of = rows.map((r) => {
		let stack = stacks.find((s) => s.group === r.group && Math.abs(s.right - r.right) <= 1);
		if (!stack) {
			stack = { group: r.group, right: r.right, width: 0 };
			stacks.push(stack);
		}
		stack.width = Math.max(stack.width, Math.ceil(r.width));
		return stack;
	});
	return of.map((s) => s.width);
}

export const money = (n: number) => `$${n.toFixed(2)}`;

export function resetIn(resets: string | null | undefined, now: number): string | null {
	const at = resets ? Date.parse(resets) : Number.NaN;
	if (!Number.isFinite(at) || at <= now) return null;
	return countdown(at - now);
}

/** Largest unit only: the readout column holds "100% · resets 5d". */
export function resetInShort(resets: string | null | undefined, now: number): string | null {
	const at = resets ? Date.parse(resets) : Number.NaN;
	if (!Number.isFinite(at) || at <= now) return null;
	const mins = Math.max(1, Math.round((at - now) / 60_000));
	if (mins < 60) return `${mins}m`;
	const hours = Math.floor(mins / 60);
	if (hours < 24) return `${hours}h`;
	return `${Math.floor(hours / 24)}d`;
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
