import { describe, expect, it } from 'vitest';
import type { OAuthAccount } from '$lib/queries';
import {
	accountAdapters,
	accountBacksAdapter,
	adapterForProvider,
	allAdapters,
	compatiblePools,
	effectiveAdapterFor,
	providerForAdapter,
	staleAccountPick
} from '$lib/components/organisms/spawn/options';
import { headlinePct } from '$lib/components/molecules/usage-battery.logic';
import { parityFixture } from './fixtures';

type Providers = string[] | null;

type Fixture = {
	allAdapters: string[];
	adapterForProvider: { provider: string; out: string }[];
	accountAdapters: { providers: string[]; out: string[] }[];
	accountBacksAdapter: { providers: Providers; adapter: string; out: boolean }[];
	effectiveAdapterFor: { providers: Providers; adapter: string; out: string }[];
	providerForAdapter: { providers: string[]; adapter: string; out: string | null }[];
	staleAccountPick: { value: string; names: string[]; out: boolean }[];
	compatiblePools: {
		accounts: { key: string; providers: string[] }[];
		cases: { pools: string[][]; harness: string; out: number[] }[];
	};
	envKeyValid: { key: string; out: boolean }[];
	headlinePct: { windows: { key: string; utilization: number }[]; out: number | null }[];
};

const fx = parityFixture<Fixture>('spawnAccounts');

// The crate ports `^[A-Z_][A-Z0-9_]*$` as a hand-rolled check; the form uses
// the regex. Both must accept the same keys.
const ENV_KEY_RE = /^[A-Z_][A-Z0-9_]*$/;

const acct = (providers: string[], name = 'acct'): OAuthAccount =>
	({ id: name, name, providers: providers.map((provider) => ({ provider })) }) as OAuthAccount;

const maybeAcct = (providers: Providers): OAuthAccount | undefined =>
	providers === null ? undefined : acct(providers);

describe('spawn account parity fixtures', () => {
	it('adapter list', () => {
		expect(allAdapters).toEqual(fx.allAdapters);
	});

	it('adapterForProvider', () => {
		for (const c of fx.adapterForProvider)
			expect(adapterForProvider(c.provider), JSON.stringify(c)).toBe(c.out);
	});

	it('accountAdapters', () => {
		for (const c of fx.accountAdapters)
			expect(accountAdapters(acct(c.providers)), JSON.stringify(c)).toEqual(c.out);
	});

	it('accountBacksAdapter', () => {
		for (const c of fx.accountBacksAdapter)
			expect(accountBacksAdapter(maybeAcct(c.providers), c.adapter), JSON.stringify(c)).toBe(
				c.out
			);
	});

	it('effectiveAdapterFor', () => {
		for (const c of fx.effectiveAdapterFor)
			expect(effectiveAdapterFor(maybeAcct(c.providers), c.adapter), JSON.stringify(c)).toBe(
				c.out
			);
	});

	it('providerForAdapter', () => {
		for (const c of fx.providerForAdapter)
			expect(
				providerForAdapter(acct(c.providers), c.adapter)?.provider ?? null,
				JSON.stringify(c)
			).toBe(c.out);
	});

	it('staleAccountPick', () => {
		for (const c of fx.staleAccountPick)
			expect(
				staleAccountPick(
					c.value,
					c.names.map((n) => acct(['anthropic'], n))
				),
				JSON.stringify(c)
			).toBe(c.out);
	});

	it('compatiblePools', () => {
		const accounts = fx.compatiblePools.accounts.map((a) => acct(a.providers, a.key));
		for (const c of fx.compatiblePools.cases) {
			const pools = c.pools.map((members, i) => ({
				id: `p${i}`,
				members: members.map((account_id) => ({ account_id }))
			}));
			const kept = compatiblePools(pools as never[], accounts, c.harness).map(
				(p: { id: string }) => Number(p.id.slice(1))
			);
			expect(kept, JSON.stringify(c)).toEqual(c.out);
		}
	});

	it('headlinePct', () => {
		for (const c of fx.headlinePct)
			expect(headlinePct(c.windows as never[]), JSON.stringify(c)).toBe(c.out);
	});

	it('envKeyValid', () => {
		for (const c of fx.envKeyValid) expect(ENV_KEY_RE.test(c.key), JSON.stringify(c)).toBe(c.out);
	});
});
