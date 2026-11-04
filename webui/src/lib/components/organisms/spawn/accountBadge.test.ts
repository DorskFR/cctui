import { describe, expect, it } from 'vitest';
import type { AccountPoolView } from '@bindings/AccountPoolView';
import type { OAuthAccount } from '$lib/queries';
import { hashHue } from '$lib/format';
import { badgeLook } from './accountBadge';

const accounts = [
	{ id: 'a1', name: 'personal', emoji: '🐼' },
	{ id: 'a2', name: 'work', emoji: null },
	{ id: 'a3', name: 'ébène', emoji: '  ' }
] as unknown as OAuthAccount[];
const pools = [{ id: 'pool1', name: 'shared', members: [] }] as unknown as AccountPoolView[];
const spec = (s: Partial<{ account_id: string; pool_id: string; no_account: boolean }>) => ({
	account_id: null,
	pool_id: null,
	no_account: false,
	...s
});

describe('badgeLook', () => {
	it('shows the account emoji, tinted by the account id', () => {
		expect(badgeLook(spec({ account_id: 'a1' }), accounts, pools)).toEqual({
			kind: 'account',
			glyph: '🐼',
			hue: hashHue('a1')
		});
	});

	it("falls back to the account's upper-cased initial without an emoji", () => {
		expect(badgeLook(spec({ account_id: 'a2' }), accounts, pools).glyph).toBe('W');
		expect(badgeLook(spec({ account_id: 'a3' }), accounts, pools).glyph).toBe('É');
	});

	it('gives a pool its own initial and a hue distinct from an account with the same id', () => {
		const look = badgeLook(spec({ pool_id: 'pool1' }), accounts, pools);
		expect(look).toEqual({ kind: 'pool', glyph: 'S', hue: hashHue('pool:pool1') });
	});

	it('is a neutral ? for no account, Auto and a stale account or pool', () => {
		const none = { kind: 'none', glyph: '?', hue: null };
		expect(badgeLook(spec({ no_account: true, account_id: 'a1' }), accounts, pools)).toEqual(none);
		expect(badgeLook(spec({}), accounts, pools)).toEqual(none);
		expect(badgeLook(spec({ account_id: 'gone' }), accounts, pools)).toEqual(none);
		expect(badgeLook(spec({ pool_id: 'gone' }), accounts, pools)).toEqual(none);
	});

	it('keeps the same tint across calls', () => {
		const a = badgeLook(spec({ account_id: 'a2' }), accounts, pools);
		const b = badgeLook(spec({ account_id: 'a2' }), [...accounts].reverse(), pools);
		expect(a.hue).toBe(b.hue);
	});
});
