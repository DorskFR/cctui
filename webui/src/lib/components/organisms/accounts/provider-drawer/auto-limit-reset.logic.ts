/** The `auto_limit_reset` policy stored on a provider's `provider_settings`.
 *  Mirrors the server's reading: anything absent or malformed falls back to a
 *  default, and the default is off. */
export type AutoLimitReset = {
	enabled: boolean;
	/** Codex: redeem once the 5h window is this used. */
	used_pct: number;
	/** Codex: redeem a credit expiring within this many hours regardless. */
	expires_within_hours: number;
	/** Claude: never redeem while the weekly window is at or past this. */
	weekly_max_pct: number;
};

export const AUTO_LIMIT_RESET_KEY = 'auto_limit_reset';

export const AUTO_LIMIT_RESET_DEFAULT: AutoLimitReset = {
	enabled: false,
	used_pct: 90,
	expires_within_hours: 24,
	weekly_max_pct: 80
};

const num = (v: unknown, fallback: number): number => {
	const n = Number(v);
	return v !== null && v !== undefined && v !== '' && Number.isFinite(n) && n >= 0 ? n : fallback;
};

export function readAutoLimitReset(settings: Record<string, unknown> | null | undefined): AutoLimitReset {
	const raw = settings?.[AUTO_LIMIT_RESET_KEY];
	if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return { ...AUTO_LIMIT_RESET_DEFAULT };
	const r = raw as Record<string, unknown>;
	return {
		enabled: r.enabled === true,
		used_pct: num(r.used_pct, AUTO_LIMIT_RESET_DEFAULT.used_pct),
		expires_within_hours: num(r.expires_within_hours, AUTO_LIMIT_RESET_DEFAULT.expires_within_hours),
		weekly_max_pct: num(r.weekly_max_pct, AUTO_LIMIT_RESET_DEFAULT.weekly_max_pct)
	};
}

/** `provider_settings` with the policy written in. Off with every knob at its
 *  default drops the key, so an untouched provider row stays as it was. */
export function writeAutoLimitReset(
	settings: Record<string, unknown>,
	policy: AutoLimitReset
): Record<string, unknown> {
	const { [AUTO_LIMIT_RESET_KEY]: _, ...rest } = settings;
	const clean: AutoLimitReset = {
		enabled: policy.enabled,
		used_pct: num(policy.used_pct, AUTO_LIMIT_RESET_DEFAULT.used_pct),
		expires_within_hours: num(policy.expires_within_hours, AUTO_LIMIT_RESET_DEFAULT.expires_within_hours),
		weekly_max_pct: num(policy.weekly_max_pct, AUTO_LIMIT_RESET_DEFAULT.weekly_max_pct)
	};
	const isDefault = (Object.keys(clean) as (keyof AutoLimitReset)[]).every(
		(k) => clean[k] === AUTO_LIMIT_RESET_DEFAULT[k]
	);
	return isDefault ? rest : { ...rest, [AUTO_LIMIT_RESET_KEY]: clean };
}
