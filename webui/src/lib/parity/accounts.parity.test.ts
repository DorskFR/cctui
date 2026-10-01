import { describe, expect, it } from 'vitest';
import type { AccountPoolView } from '@bindings/AccountPoolView';
import type { AccountRedirect } from '@bindings/AccountRedirect';
import type { OAuthAccount } from '$lib/queries';
import { redirectChipsFor } from '$lib/queries/accounts';
import {
	acceptsDrop,
	membershipAfterMove,
	poolOf
} from '$lib/components/organisms/accounts/pools.logic';
import { parityFixture } from './fixtures';

type Fixture = {
	accounts: { id: string; name: string; user_id: string; pool_eligible: boolean }[];
	pools: { id: string; user_id: string; members: { account_id: string; position: number }[] }[];
	redirects: {
		id: string;
		from_account: string;
		to_account: string | null;
		family: string;
		expires_at: string | null;
	}[];
	poolOf: { accountId: string; out: string | null }[];
	acceptsMember: { poolId: string; accountId: string; out: boolean }[];
	membershipAfterMove: {
		accountId: string;
		to: string | null;
		out: { poolId: string; accounts: string[] }[];
	}[];
	redirectChips: {
		accountId: string;
		out: { id: string; family: string; targetName: string; until: string | null }[];
	}[];
};

const fx = parityFixture<Fixture>('accounts');

const accounts = fx.accounts as unknown as OAuthAccount[];
const pools = fx.pools as unknown as AccountPoolView[];
const redirects = fx.redirects as unknown as AccountRedirect[];
const pool = (id: string) => pools.find((p) => p.id === id)!;

describe('accounts parity fixtures', () => {
	it('poolOf', () => {
		for (const c of fx.poolOf)
			expect(poolOf(pools, c.accountId)?.id ?? null, JSON.stringify(c)).toBe(c.out);
	});
	it('acceptsMember', () => {
		for (const c of fx.acceptsMember)
			expect(acceptsDrop(pool(c.poolId), c.accountId, accounts), JSON.stringify(c)).toBe(c.out);
	});
	it('membershipAfterMove', () => {
		for (const c of fx.membershipAfterMove)
			expect(
				membershipAfterMove(pools, c.accountId, c.to === null ? null : pool(c.to)),
				JSON.stringify(c)
			).toEqual(c.out);
	});
	it('redirectChips', () => {
		for (const c of fx.redirectChips)
			expect(redirectChipsFor(redirects, accounts, c.accountId), JSON.stringify(c)).toEqual(c.out);
	});
});
