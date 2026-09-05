import type { AccountPoolView } from '@bindings/AccountPoolView';
import type { AccountUsageEntry, OAuthAccount, UsageWindow } from '$lib/queries';

export const ACCOUNT_DRAG_MIME = 'application/x-cctui-account';

export interface PoolGroup {
	pool: AccountPoolView;
	accounts: OAuthAccount[];
}

/** The pool an account sits in, if any. An account belongs to at most one
 *  pool; the first match wins should the server ever disagree. */
export function poolOf(pools: AccountPoolView[], accountId: string): AccountPoolView | null {
	return pools.find((p) => p.members.some((mem) => mem.account_id === accountId)) ?? null;
}

export function groupAccounts(
	accounts: OAuthAccount[],
	pools: AccountPoolView[]
): { pooled: PoolGroup[]; solo: OAuthAccount[] } {
	const byId = new Map(accounts.map((a) => [a.id, a]));
	const placed = new Set<string>();
	const pooled = pools.map((pool) => {
		const members = [...pool.members]
			.sort((x, y) => x.position - y.position)
			.map((mem) => byId.get(mem.account_id))
			.filter((a): a is OAuthAccount => !!a && !placed.has(a.id));
		for (const a of members) placed.add(a.id);
		return { pool, accounts: members };
	});
	return { pooled, solo: accounts.filter((a) => !placed.has(a.id)) };
}

/** Whether a dragged account may land in this pool: it must exist, not be a
 *  member already, and be the pool owner's own or not withheld from pools. */
export function acceptsDrop(
	pool: Pick<AccountPoolView, 'user_id' | 'members'>,
	accountId: string,
	accounts: OAuthAccount[]
): boolean {
	const a = accounts.find((x) => x.id === accountId);
	if (!a) return false;
	if (pool.members.some((mem) => mem.account_id === accountId)) return false;
	return a.user_id === pool.user_id || a.pool_eligible;
}

export interface MembershipChange {
	poolId: string;
	accounts: string[];
}

/** The membership PATCHes a move implies: the account leaves its current pool
 *  (if any) and joins `to` (null ⇒ just leaves). Order inside a pool is kept. */
export function membershipAfterMove(
	pools: AccountPoolView[],
	accountId: string,
	to: AccountPoolView | null
): MembershipChange[] {
	const from = poolOf(pools, accountId);
	if (from?.id === to?.id) return [];
	const ids = (p: AccountPoolView) =>
		[...p.members].sort((x, y) => x.position - y.position).map((mem) => mem.account_id);
	const out: MembershipChange[] = [];
	if (from) out.push({ poolId: from.id, accounts: ids(from).filter((id) => id !== accountId) });
	if (to) out.push({ poolId: to.id, accounts: [...ids(to), accountId] });
	return out;
}

export interface ExhaustedWindow {
	provider: string;
	window: UsageWindow;
}

export function exhaustedWindow(
	entries: AccountUsageEntry[] | null | undefined,
	accountId: string
): ExhaustedWindow | null {
	for (const e of entries ?? []) {
		if (e.account !== accountId) continue;
		const w = e.windows.find((x) => x.utilization >= 100);
		if (w) return { provider: e.provider, window: w };
	}
	return null;
}
