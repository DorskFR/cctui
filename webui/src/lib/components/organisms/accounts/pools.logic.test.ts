import { describe, expect, it } from 'vitest';
import type { AccountPoolView } from '@bindings/AccountPoolView';
import type { AccountUsageEntry, OAuthAccount } from '$lib/queries';
import {
	acceptsDrop,
	exhaustedWindow,
	groupAccounts,
	membershipAfterMove,
	poolOf
} from './pools.logic';

const acct = (id: string, user_id = 'u1', pool_eligible = true): OAuthAccount =>
	({ id, name: id, user_id, pool_eligible, providers: [] }) as unknown as OAuthAccount;

const pool = (id: string, members: string[], user_id = 'u1'): AccountPoolView => ({
	id,
	user_id,
	name: id,
	strategy: 'headroom',
	failover: false,
	created_at: '',
	members: members.map((account_id, position) => ({
		account_id,
		name: account_id,
		position,
		owned: true,
		pool_eligible: true
	}))
});

const accounts = [acct('a'), acct('b'), acct('c'), acct('d', 'u2', false), acct('e', 'u2')];
const pools = [pool('p1', ['b', 'a']), pool('p2', ['c'])];

describe('poolOf / groupAccounts', () => {
	it('finds the pool holding an account', () => {
		expect(poolOf(pools, 'a')?.id).toBe('p1');
		expect(poolOf(pools, 'd')).toBeNull();
	});

	it('groups members in ladder order and leaves the rest solo', () => {
		const g = groupAccounts(accounts, pools);
		expect(g.pooled.map((x) => x.accounts.map((a) => a.id))).toEqual([['b', 'a'], ['c']]);
		expect(g.solo.map((a) => a.id)).toEqual(['d', 'e']);
	});

	it('places an account in one pool only when the server lists it twice', () => {
		const g = groupAccounts(accounts, [pool('p1', ['a']), pool('p2', ['a'])]);
		expect(g.pooled[0].accounts.map((a) => a.id)).toEqual(['a']);
		expect(g.pooled[1].accounts).toEqual([]);
	});
});

describe('acceptsDrop', () => {
	it('refuses a member, an unknown id, and a withheld foreign account', () => {
		expect(acceptsDrop(pools[0], 'a', accounts)).toBe(false);
		expect(acceptsDrop(pools[0], 'zz', accounts)).toBe(false);
		expect(acceptsDrop(pools[0], 'd', accounts)).toBe(false);
	});

	it('accepts the owner’s account and an eligible shared one', () => {
		expect(acceptsDrop(pools[0], 'c', accounts)).toBe(true);
		expect(acceptsDrop(pools[0], 'e', accounts)).toBe(true);
	});
});

describe('membershipAfterMove', () => {
	it('leaves the old pool and appends to the new one', () => {
		expect(membershipAfterMove(pools, 'a', pools[1])).toEqual([
			{ poolId: 'p1', accounts: ['b'] },
			{ poolId: 'p2', accounts: ['c', 'a'] }
		]);
	});

	it('only joins when the account was solo, only leaves when dropped nowhere', () => {
		expect(membershipAfterMove(pools, 'd', pools[1])).toEqual([
			{ poolId: 'p2', accounts: ['c', 'd'] }
		]);
		expect(membershipAfterMove(pools, 'c', null)).toEqual([{ poolId: 'p2', accounts: [] }]);
	});

	it('is a no-op for a drop into the same pool', () => {
		expect(membershipAfterMove(pools, 'a', pools[0])).toEqual([]);
	});
});

describe('exhaustedWindow', () => {
	const entry = (account: string, provider: string, utilization: number): AccountUsageEntry =>
		({
			account,
			provider,
			windows: [{ key: 'session', kind: 'session', label: '5h', utilization }]
		}) as unknown as AccountUsageEntry;

	it('returns the first window at 100% for the account, else null', () => {
		const entries = [
			entry('a', 'openai', 40),
			entry('a', 'anthropic', 100),
			entry('b', 'anthropic', 100)
		];
		expect(exhaustedWindow(entries, 'a')?.provider).toBe('anthropic');
		expect(exhaustedWindow(entries, 'c')).toBeNull();
		expect(exhaustedWindow(null, 'a')).toBeNull();
	});
});
