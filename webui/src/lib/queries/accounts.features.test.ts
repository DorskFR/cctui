// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { setCapabilitiesForTest } from '$lib/features.svelte';

const queries = vi.hoisted(() => [] as { queryKey: readonly unknown[]; enabled?: boolean }[]);

vi.mock('@tanstack/svelte-query', () => ({
	createQuery: (opts: () => { queryKey: readonly unknown[]; enabled?: boolean }) => {
		queries.push(opts());
		return {};
	},
	useQueryClient: () => ({ invalidateQueries: vi.fn(), setQueryData: vi.fn() })
}));

import { useServiceTokens } from './accounts';
import accountPage from '../../routes/accounts/[id]/+page.svelte?raw';

afterEach(() => {
	queries.length = 0;
	setCapabilitiesForTest(null);
});

describe('service tokens query', () => {
	it('asks nothing without service tokens', () => {
		setCapabilitiesForTest({ features: [], switchable: ['service_tokens'] });
		useServiceTokens(() => 'a1');
		expect(queries.at(-1)?.enabled).toBe(false);
	});

	it('lists an account’s tokens with service tokens on', () => {
		setCapabilitiesForTest({ features: ['service_tokens'], switchable: ['service_tokens'] });
		useServiceTokens(() => 'a1');
		expect(queries.at(-1)).toMatchObject({ queryKey: ['service-tokens', 'a1'], enabled: true });
	});

	it('mounts the account page section only with service tokens on', () => {
		expect(accountPage).toMatch(
			/\{#if hasFeature\('service_tokens'\)\}\s*<ServiceTokensSection \{account\} \/>\s*\{\/if\}/
		);
	});
});
