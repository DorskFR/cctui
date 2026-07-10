import { describe, expect, it } from 'vitest';
import type { AccountProvider, OAuthAccount } from '$lib/queries';
import {
	accountAdapters,
	accountBacksAdapter,
	adapterForProvider,
	effectiveAdapterFor,
	providerForAdapter,
	withAliasTargets
} from './options';

const provider = (p: string): AccountProvider => ({ provider: p, id: `id-${p}` }) as AccountProvider;
const account = (...providers: string[]): OAuthAccount =>
	({ name: 'acct', providers: providers.map(provider) }) as OAuthAccount;

describe('adapterForProvider', () => {
	it('maps the openai family to codex, everything else to claude-code', () => {
		expect(adapterForProvider('openai')).toBe('codex');
		expect(adapterForProvider('openai-compatible')).toBe('codex');
		expect(adapterForProvider('anthropic')).toBe('claude-code');
		expect(adapterForProvider('anthropic-compatible')).toBe('claude-code');
	});
});

describe('accountAdapters', () => {
	it('is the provider-family union, in stable order', () => {
		expect(accountAdapters(account('anthropic', 'openai'))).toEqual(['claude-code', 'codex']);
		expect(accountAdapters(account('openai', 'anthropic-compatible'))).toEqual([
			'claude-code',
			'codex'
		]);
		expect(accountAdapters(account('anthropic'))).toEqual(['claude-code']);
		expect(accountAdapters(account('openai-compatible'))).toEqual(['codex']);
		expect(accountAdapters(account())).toEqual([]);
	});
});

describe('providerForAdapter', () => {
	it('returns the credential backing the harness family', () => {
		const a = account('anthropic', 'openai');
		expect(providerForAdapter(a, 'claude-code')?.provider).toBe('anthropic');
		expect(providerForAdapter(a, 'codex')?.provider).toBe('openai');
	});
	it('is undefined without an account or a matching family', () => {
		expect(providerForAdapter(undefined, 'codex')).toBeUndefined();
		expect(providerForAdapter(account('anthropic'), 'codex')).toBeUndefined();
	});
});

describe('effectiveAdapterFor', () => {
	it('keeps the user pick with no account', () => {
		expect(effectiveAdapterFor(undefined, 'codex')).toBe('codex');
	});
	it('keeps the user pick when the account offers that family', () => {
		expect(effectiveAdapterFor(account('anthropic', 'openai'), 'codex')).toBe('codex');
	});
	it('falls back to the account first family when the pick is not offered', () => {
		expect(effectiveAdapterFor(account('anthropic'), 'codex')).toBe('claude-code');
		expect(effectiveAdapterFor(account('openai-compatible'), 'claude-code')).toBe('codex');
	});
});

describe('accountBacksAdapter', () => {
	// CCT-581: the submitted harness is the user's pick, gated by this predicate,
	// NOT rewritten by effectiveAdapterFor. A named anthropic-only account that
	// can't back codex blocks the spawn instead of silently submitting claude.
	it('is true with no account (Auto / No account runs any harness)', () => {
		expect(accountBacksAdapter(undefined, 'codex')).toBe(true);
		expect(accountBacksAdapter(undefined, 'claude-code')).toBe(true);
	});
	it('is true only when the account has a provider of that family', () => {
		expect(accountBacksAdapter(account('anthropic', 'openai'), 'codex')).toBe(true);
		expect(accountBacksAdapter(account('anthropic'), 'claude-code')).toBe(true);
	});
	it('is false when the account cannot back the picked harness', () => {
		// The exact regression: anthropic-only account + codex pick. The old
		// effectiveAdapterFor would coerce this to claude-code; the predicate now
		// rejects it so the submitted adapter_id is never silently swapped.
		expect(accountBacksAdapter(account('anthropic'), 'codex')).toBe(false);
		expect(effectiveAdapterFor(account('anthropic'), 'codex')).toBe('claude-code');
		expect(accountBacksAdapter(account('openai-compatible'), 'claude-code')).toBe(false);
	});
});

describe('withAliasTargets', () => {
	it('annotates aliased families and leaves the rest untouched (CCT-415)', () => {
		const models = [
			{ v: '', label: 'Default' },
			{ v: 'opus', label: 'Opus' },
			{ v: 'sonnet', label: 'Sonnet' }
		];
		expect(withAliasTargets(models, { opus: 'claude-opus-4-8[1m]' })).toEqual([
			{ v: '', label: 'Default' },
			{ v: 'opus', label: 'Opus (claude-opus-4-8[1m])' },
			{ v: 'sonnet', label: 'Sonnet' }
		]);
		expect(withAliasTargets(models, null)).toEqual(models);
	});
});
