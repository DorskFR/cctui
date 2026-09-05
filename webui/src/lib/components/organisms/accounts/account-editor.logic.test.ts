import { describe, expect, it } from 'vitest';
import type { OAuthAccount } from '$lib/queries';
import {
	aliasObject,
	availableKinds,
	buildRateLimits,
	buildSoftLimits,
	buildUsageNotices,
	envObject,
	fwModelList,
	modelList
} from './account-editor.logic';

describe('buildSoftLimits', () => {
	it('keeps percent and dollar windows apart and drops empty ones', () => {
		expect(
			buildSoftLimits({
				session: { cap: 80, capUsd: null, bypass: '30' as unknown as number },
				weekly_all: { cap: null, capUsd: null, bypass: null },
				usd_5h: { cap: null, capUsd: 2.5, bypass: null },
				odd: { cap: 'x' as unknown as number, capUsd: null, bypass: -3 }
			})
		).toEqual({
			session: { cap_pct: 80, bypass_minutes: 30 },
			usd_5h: { cap_usd: 2.5, bypass_minutes: null },
			odd: { cap_pct: null, bypass_minutes: 0 }
		});
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
