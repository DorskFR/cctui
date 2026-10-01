import { providerFamily } from '$lib/providers';

/** A credential at or above this utilization is spent: listed, never recommended. */
export const LIMITED_PCT = 95;

export interface SwitchWindow {
	pct: number;
	resetsInSecs: number | null;
}

export interface SwitchCredential {
	/** The identity id, which is what the switch body posts alongside `family`. */
	accountId: string;
	accountName: string;
	provider: string;
	windows: SwitchWindow[];
}

export interface SwitchBinding {
	family: string;
	accountId: string;
	accountName: string;
}

export interface SwitchOption {
	accountId: string;
	accountName: string;
	provider: string;
	pct: number | null;
	resetsInSecs: number | null;
	current: boolean;
	limited: boolean;
}

const worst = (windows: SwitchWindow[]): [number | null, number | null] => {
	let found: SwitchWindow | null = null;
	for (const w of windows) if (!found || w.pct > found.pct) found = w;
	return found ? [found.pct, found.resetsInSecs] : [null, null];
};

const cmp = (a: number, b: number) => (a < b ? -1 : a > b ? 1 : 0);

export function switchOptions(
	binding: SwitchBinding,
	credentials: SwitchCredential[],
	limitedPct: number = LIMITED_PCT
): SwitchOption[] {
	const rows = credentials
		.filter((c) => providerFamily(c.provider) === binding.family)
		.map((c) => {
			const [pct, resetsInSecs] = worst(c.windows);
			return {
				accountId: c.accountId,
				accountName: c.accountName,
				provider: c.provider,
				pct,
				resetsInSecs,
				current: c.accountId === binding.accountId,
				limited: pct !== null && pct >= limitedPct
			};
		});
	rows.sort(
		(a, b) =>
			cmp(Number(b.current), Number(a.current)) ||
			cmp(Number(a.limited), Number(b.limited)) ||
			cmp(a.pct ?? Number.MAX_VALUE, b.pct ?? Number.MAX_VALUE) ||
			cmp(a.resetsInSecs ?? Number.MAX_SAFE_INTEGER, b.resetsInSecs ?? Number.MAX_SAFE_INTEGER) ||
			(a.accountName < b.accountName ? -1 : a.accountName > b.accountName ? 1 : 0)
	);
	return rows;
}

export function recommended(options: SwitchOption[]): number | null {
	const best = options.findIndex((o) => !o.current && !o.limited);
	if (best >= 0) return best;
	const any = options.findIndex((o) => !o.current);
	return any >= 0 ? any : null;
}
