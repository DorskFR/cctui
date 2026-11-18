import { describe, expect, it } from 'vitest';
import type { OAuthAccount } from '@bindings/OAuthAccount';
import { ownedAccounts, sharedWithMe } from './accountSettings.logic';

const account = (id: string, userId: string, name: string) =>
	({ id, user_id: userId, name, pool_eligible: true }) as unknown as OAuthAccount;

describe('account ownership for Settings › Accounts', () => {
	const list = [account('a2', 'u-me', 'zeta'), account('a1', 'u-me', 'alpha'), account('b1', 'u-bob', 'bob max')];

	it('splits what I own from what is shared with me, by name', () => {
		expect(ownedAccounts(list, 'u-me').map((a) => a.id)).toEqual(['a1', 'a2']);
		expect(sharedWithMe(list, 'u-me').map((a) => a.id)).toEqual(['b1']);
	});

	it('claims nothing before the caller is known', () => {
		expect(ownedAccounts(list, null)).toEqual([]);
		expect(sharedWithMe(list, undefined)).toEqual([]);
		expect(ownedAccounts(undefined, 'u-me')).toEqual([]);
	});
});
