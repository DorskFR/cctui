// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { OAuthAccount } from '#lib/queries/index.ts';

const degraded = {
	family: 'anthropic',
	indicator: 'minor',
	description: 'Elevated errors',
	url: 'https://status.example.test',
	components: [{ name: 'API', status: 'degraded_performance' }],
	incidents: []
};

vi.mock('#lib/queries/index.ts', () => ({
	useAllAccountsUsage: () => ({
		data: [
			{ account: 'acct', provider: 'anthropic', windows: [], provider_status: degraded },
			{ account: 'acct', provider: 'openai', windows: [], provider_status: degraded }
		]
	}),
	useResourceShares: () => ({ data: [] }),
	useAccountActions: () => ({})
}));
vi.mock('#lib/components/organisms/accounts/ProviderColumn.svelte', () => ({ default: () => {} }));
vi.mock('#lib/components/molecules/ResourceShares.svelte', () => ({ default: () => {} }));

const { default: AccountCard } = await import('./AccountCard.svelte');

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const account = {
	id: 'acct',
	name: 'alpha',
	emoji: null,
	user_name: 'owner',
	providers: [
		{ id: 'p1', account_id: 'acct', provider: 'anthropic', family: 'anthropic' },
		{ id: 'p2', account_id: 'acct', provider: 'openai', family: 'openai' }
	],
	created_at: '2026-01-01T00:00:00Z'
} as unknown as OAuthAccount;

describe.each([
	['dock', true],
	['accounts page', false]
])('AccountCard head (%s)', (_, compact) => {
	it('shows no per-account provider status even while a provider is degraded', () => {
		comp = mount(AccountCard, { target: document.body, props: { account, compact, showOwner: true } });
		flushSync();
		const head = document.querySelector('header.head') as HTMLElement;
		expect(head.textContent).toContain('alpha');
		expect(head.querySelector('a[href^="https://status."]')).toBeNull();
		expect(head.textContent).not.toMatch(/Claude|Codex/);
	});
});
