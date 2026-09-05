import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import type { OAuthAccount } from '$lib/queries';
import AccessAccountsTab from './AccessAccountsTab.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const account = (id: string, providers: string[]): OAuthAccount =>
	({
		id,
		name: `acct-${id}`,
		emoji: null,
		created_at: '2026-01-01T00:00:00Z',
		providers: providers.map((provider) => ({ provider }))
	}) as unknown as OAuthAccount;

describe('AccessAccountsTab renders a stacked kit DataTable', () => {
	it('gives every column a card role and puts the link in the actions cell', () => {
		comp = mount(AccessAccountsTab, {
			target: document.body,
			props: { accounts: [account('a', ['github']), account('b', [])] }
		});
		const table = document.querySelector('[data-tsu="DataTable"]');
		expect(table?.classList.contains('dt-stack')).toBe(true);
		const rows = [...document.querySelectorAll('tbody tr[data-part="row"]')];
		expect(rows).toHaveLength(2);
		const roles = [...rows[0].querySelectorAll('td[data-part="cell"]')].map((td) => td.getAttribute('data-role'));
		expect(roles).toEqual(['title', 'detail', 'meta']);
		expect(rows[0].querySelector('td[data-role="title"]')?.textContent).toContain('acct-a');
		expect(rows[1].querySelector('td[data-role="detail"]')?.textContent).toContain('no providers');
		expect(rows[0].querySelector('td.dt-actions a')?.getAttribute('href')).toBe('/accounts');
	});

	it('shows the empty copy when there are no accounts', () => {
		comp = mount(AccessAccountsTab, { target: document.body, props: { accounts: [] } });
		expect(document.querySelector('tr[data-part="empty"]')?.textContent).toContain('No accounts');
	});
});
