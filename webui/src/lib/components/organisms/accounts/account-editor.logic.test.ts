import { describe, expect, it } from 'vitest';
import type { OAuthAccount } from '#lib/queries/index.ts';
import {
	aliasObject,
	availableKinds,
	buildRateLimits,
	buildSoftLimits,
	buildUsageNotices,
	canEditProvider,
	envObject,
	fwModelList,
	modelList,
	ownsAccount
} from './account-editor.logic';

describe('buildSoftLimits', () => {
	it('keeps percent and dollar windows apart and drops empty ones', () => {
		expect(
			buildSoftLimits({
				session: { cap: 80, capUsd: null, bypass: '30' as unknown as number, paceCap: null },
				weekly_all: { cap: null, capUsd: null, bypass: null, paceCap: null },
				usd_5h: { cap: null, capUsd: 2.5, bypass: null, paceCap: null },
				odd: { cap: 'x' as unknown as number, capUsd: null, bypass: -3, paceCap: null }
			})
		).toEqual({
			session: { cap_pct: 80, bypass_minutes: 30, pace_cap: null },
			usd_5h: { cap_usd: 2.5, bypass_minutes: null },
			odd: { cap_pct: null, bypass_minutes: 0, pace_cap: null }
		});
	});

	it('carries a pace cap alone and rejects a non-positive one', () => {
		expect(
			buildSoftLimits({
				session: { cap: null, capUsd: null, bypass: null, paceCap: 1.5 },
				weekly_all: { cap: null, capUsd: null, bypass: null, paceCap: 0 },
				usd_5h: { cap: null, capUsd: null, bypass: null, paceCap: 2 }
			})
		).toEqual({ session: { cap_pct: null, bypass_minutes: null, pace_cap: 1.5 } });
	});
});

describe('rate limits and notices', () => {
	it('normalises blanks to unlimited and clamps the step', () => {
		expect(buildRateLimits({ rpm: '' as unknown as number, tpm: 90000 })).toEqual({
			rpm: null,
			tpm: 90000
		});
		expect(buildUsageNotices({ enabled: true, step_pct: 0 })).toEqual({ enabled: true, step_pct: 10 });
		expect(buildUsageNotices({ enabled: false, step_pct: 25.4 })).toEqual({
			enabled: false,
			step_pct: 25
		});
	});
});

describe('row collapsers', () => {
	it('trim and drop incomplete rows', () => {
		expect(aliasObject([{ alias: ' opus ', model: 'm1' }, { alias: '', model: 'x' }])).toEqual({
			opus: 'm1'
		});
		expect(envObject([{ name: ' A ', value: '1' }, { name: '', value: '2' }])).toEqual({ A: '1' });
		expect(modelList([{ model: ' q ', label: '' }, { model: '', label: 'nope' }])).toEqual([
			{ model: 'q', label: 'q' }
		]);
		expect(fwModelList([{ model: 'f', label: ' F ', extra: 1 } as never])).toEqual([
			{ model: 'f', label: 'F', extra: 1 }
		]);
	});
});

describe('availableKinds', () => {
	it('hides every kind of a family the account already holds', () => {
		const a = { providers: [{ family: 'anthropic' }] } as unknown as OAuthAccount;
		const kinds = availableKinds(a);
		expect(kinds).not.toContain('anthropic');
		expect(kinds).not.toContain('anthropic-compatible');
		expect(kinds).toContain('openai');
	});
});

describe('ownsAccount', () => {
	const account = { user_id: 'owner' };

	it('lets the owner edit their account', () => {
		expect(ownsAccount(account, { role: 'user', user_id: 'owner' })).toBe(true);
	});

	it('keeps an account shared with another user read-only', () => {
		expect(ownsAccount(account, { role: 'user', user_id: 'guest' })).toBe(false);
	});

	it('lets an admin edit any account', () => {
		expect(ownsAccount(account, { role: 'admin', user_id: 'root' })).toBe(true);
	});

	it('is read-only until the signed-in user is known', () => {
		expect(ownsAccount(account, undefined)).toBe(false);
		expect(ownsAccount(account, { role: 'user', user_id: null })).toBe(false);
	});
});

describe('canEditProvider', () => {
	it('offers caps on an owned, unmanaged provider', () => {
		expect(canEditProvider({ managed: false }, true)).toBe(true);
	});

	it('locks caps on a shared account', () => {
		expect(canEditProvider({ managed: false }, false)).toBe(false);
	});

	it('locks caps on a managed provider even for the owner', () => {
		expect(canEditProvider({ managed: true }, true)).toBe(false);
	});
});
