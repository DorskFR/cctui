import { describe, expect, it } from 'vitest';
import type { AccountProvider, OAuthAccount } from '$lib/queries';
import {
	ACCOUNTS_HREF,
	accountHref,
	editableProviders,
	pageParam,
	pickProvider
} from './account-nav.logic';

const prov = (id: string, managed = false) => ({ id, managed }) as unknown as AccountProvider;
const acct = (providers: AccountProvider[]) => ({ id: 'a1', providers }) as unknown as OAuthAccount;

describe('accountHref', () => {
	it('the account gear lands on the settings page with no provider selected', () => {
		expect(accountHref('a1')).toBe('/accounts/a1');
	});

	it('a provider gear lands on the same page with that provider selected', () => {
		expect(accountHref('a1', 'p1')).toBe('/accounts/a1?provider=p1');
	});

	it('carries a section only alongside a provider', () => {
		expect(accountHref('a1', 'p1', 'limits')).toBe('/accounts/a1?provider=p1&page=limits');
		expect(accountHref('a1', undefined, 'limits')).toBe('/accounts/a1');
	});

	it('back goes to the accounts list', () => {
		expect(ACCOUNTS_HREF).toBe('/accounts');
	});
});

describe('editableProviders', () => {
	it('drops managed providers', () => {
		expect(editableProviders(acct([prov('p1'), prov('p2', true)]), false).map((p) => p.id)).toEqual([
			'p1'
		]);
	});

	it('is empty for a managed account', () => {
		expect(editableProviders(acct([prov('p1')]), true)).toEqual([]);
	});
});

describe('pickProvider', () => {
	const list = [prov('p1'), prov('p2')];

	it('selects the requested provider', () => {
		expect(pickProvider(list, 'p2')?.id).toBe('p2');
	});

	it('falls back to the first one when the request is missing or unknown', () => {
		expect(pickProvider(list, null)?.id).toBe('p1');
		expect(pickProvider(list, 'gone')?.id).toBe('p1');
	});

	it('selects nothing when no provider is editable', () => {
		expect(pickProvider([], 'p1')).toBeUndefined();
	});
});

describe('pageParam', () => {
	it('accepts a known section and rejects anything else', () => {
		expect(pageParam('resets')).toBe('resets');
		expect(pageParam('nope')).toBeUndefined();
		expect(pageParam(null)).toBeUndefined();
	});
});
