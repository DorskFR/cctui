import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import type { UserRow } from '@bindings/UserRow';
import type { MachineRow } from '@bindings/MachineRow';
import type { UserTokenRow } from '@bindings/UserTokenRow';

const machines = { isLoading: true, data: undefined as MachineRow[] | undefined };
const tokens = { isLoading: false, data: [] as UserTokenRow[] };
vi.mock('$lib/queries', () => ({
	useMachines: () => machines,
	useTokens: () => tokens,
	useUserActions: () => ({})
}));
vi.mock('$lib/toast.svelte', () => ({ toasts: { ok: vi.fn(), err: vi.fn() } }));

import UserExpand from './UserExpand.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const user = { id: 'u1', name: 'ann', revoked_at: null } as unknown as UserRow;
const token = (id: string, revoked_at: string | null): UserTokenRow =>
	({ id, label: `tok-${id}`, created_at: '2026-01-01T00:00:00Z', expires_at: null, revoked_at, token_preview: 'ab…' }) as UserTokenRow;

describe('UserExpand DataTables', () => {
	it('delegates the loading state to DataTable', () => {
		machines.isLoading = true;
		comp = mount(UserExpand, { target: document.body, props: { user, onsecret: () => {} } });
		const tables = document.querySelectorAll('[data-tsu="DataTable"]');
		expect(tables).toHaveLength(2);
		expect(tables[0].getAttribute('aria-busy')).toBe('true');
		expect(tables[0].querySelector('tr[data-part="loading"]')).not.toBeNull();
		expect(document.querySelector('.spin')).toBeNull();
	});

	it('renders token rows with rowActions and a danger tone on revoked ones', () => {
		machines.isLoading = false;
		machines.data = [];
		tokens.data = [token('a', null), token('b', '2026-02-01T00:00:00Z')];
		comp = mount(UserExpand, { target: document.body, props: { user, onsecret: () => {} } });
		const rows = [...document.querySelectorAll('[data-tsu="DataTable"]')[1].querySelectorAll('tbody tr')];
		expect(rows).toHaveLength(2);
		expect(rows[0].getAttribute('data-tone')).toBeNull();
		expect(rows[1].getAttribute('data-tone')).toBe('danger');
		const live = [...rows[0].querySelectorAll('td:last-child button')].map((b) => b.textContent?.trim());
		expect(live).toEqual(['Relabel', 'Revoke']);
		const revoked = [...rows[1].querySelectorAll('td:last-child button')].map((b) => b.textContent?.trim());
		expect(revoked).toEqual(['Delete']);
	});
});
