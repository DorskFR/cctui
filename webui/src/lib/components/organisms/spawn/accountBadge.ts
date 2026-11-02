import type { AccountPoolView } from '@bindings/AccountPoolView';
import type { OAuthAccount } from '$lib/queries';
import { hashHue } from '$lib/format';
import type { ProfileSpecForm } from './profiles';

export interface BadgeLook {
	kind: 'account' | 'pool' | 'none';
	glyph: string;
	/** `null` paints the neutral tint. */
	hue: number | null;
}

const NONE: BadgeLook = { kind: 'none', glyph: '?', hue: null };

const initial = (name: string) => [...name.trim()][0]?.toLocaleUpperCase() ?? '?';

/** The badge for a spec's account pick: the account's emoji or initial, the
 *  pool's initial, or a neutral `?` for Auto, no account or a stale id. The
 *  hue hashes the id, so it holds across sessions and renames. */
export function badgeLook(
	spec: Pick<ProfileSpecForm, 'account_id' | 'pool_id' | 'no_account'>,
	accounts: readonly OAuthAccount[],
	pools: readonly AccountPoolView[]
): BadgeLook {
	if (spec.no_account) return NONE;
	const pool = spec.pool_id ? pools.find((p) => p.id === spec.pool_id) : undefined;
	if (pool) return { kind: 'pool', glyph: initial(pool.name), hue: hashHue(`pool:${pool.id}`) };
	const account = spec.account_id ? accounts.find((a) => a.id === spec.account_id) : undefined;
	if (!account) return NONE;
	return {
		kind: 'account',
		glyph: account.emoji?.trim() || initial(account.name),
		hue: hashHue(account.id)
	};
}
