import type { AutoFollowupOverride } from '@bindings/AutoFollowupOverride';
import type { AutoFollowupSetting } from '@bindings/AutoFollowupSetting';
import type { AutoFollowupThreshold } from '@bindings/AutoFollowupThreshold';
import type { ThresholdUnit } from '@bindings/ThresholdUnit';

export const DEFAULT_THRESHOLD: AutoFollowupThreshold = { unit: 'tokens', value: 150_000 };

export const DEFAULT_AUTO_FOLLOWUP: AutoFollowupSetting = {
	enabled: false,
	threshold: DEFAULT_THRESHOLD
};

export const THRESHOLD_UNITS: readonly ThresholdUnit[] = ['tokens', 'percent'];

export function clampThreshold(v: unknown): AutoFollowupThreshold | null {
	const t = v as Partial<AutoFollowupThreshold> | null | undefined;
	if (!t || typeof t !== 'object' || !THRESHOLD_UNITS.includes(t.unit as ThresholdUnit)) return null;
	const value = typeof t.value === 'number' && Number.isFinite(t.value) ? Math.round(t.value) : NaN;
	if (t.unit === 'percent') return value >= 1 && value <= 100 ? { unit: 'percent', value } : null;
	return value > 0 ? { unit: 'tokens', value } : null;
}

export function clampAutoFollowup(v: unknown): AutoFollowupSetting {
	const s = v as Partial<AutoFollowupSetting> | null | undefined;
	if (!s || typeof s !== 'object') return { ...DEFAULT_AUTO_FOLLOWUP };
	return {
		enabled: s.enabled === true,
		threshold: clampThreshold(s.threshold) ?? DEFAULT_THRESHOLD
	};
}

export const INHERIT: AutoFollowupOverride = { enabled: null, threshold: null };

export function clampOverride(v: unknown): AutoFollowupOverride {
	const o = v as Partial<AutoFollowupOverride> | null | undefined;
	if (!o || typeof o !== 'object') return { ...INHERIT };
	return {
		enabled: typeof o.enabled === 'boolean' ? o.enabled : null,
		threshold: clampThreshold(o.threshold)
	};
}

export function isInherit(o: AutoFollowupOverride | null | undefined): boolean {
	return !o || (o.enabled === null && o.threshold === null);
}

export function sameOverride(
	a: AutoFollowupOverride | null | undefined,
	b: AutoFollowupOverride | null | undefined
): boolean {
	const x = clampOverride(a);
	const y = clampOverride(b);
	return (
		x.enabled === y.enabled &&
		x.threshold?.unit === y.threshold?.unit &&
		x.threshold?.value === y.threshold?.value
	);
}

export type OverridePick = 'inherit' | 'on' | 'off';

export function pickOf(enabled: boolean | null): OverridePick {
	return enabled === null ? 'inherit' : enabled ? 'on' : 'off';
}

export function enabledOf(pick: OverridePick): boolean | null {
	return pick === 'inherit' ? null : pick === 'on';
}
