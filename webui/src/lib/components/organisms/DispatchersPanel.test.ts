import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import type { UserDispatcher } from '$lib/queries';

const list = { isLoading: true, data: undefined as UserDispatcher[] | undefined };
const pools = { data: [] as { id: string; name: string }[] };
const rename = vi.fn();
vi.mock('$lib/queries', () => ({
	useUserDispatchers: () => list,
	useDispatcherActions: () => ({ enroll: vi.fn(), rename, remove: vi.fn() }),
	useAccounts: () => ({ data: [] }),
	useAccountPools: () => pools,
	primaryProvider: () => null
}));
vi.mock('$lib/toast.svelte', () => ({ toasts: { ok: vi.fn(), error: vi.fn() } }));

import DispatchersPanel from './DispatchersPanel.svelte';

let comp: ReturnType<typeof mount> | null = null;
function cleanup() {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
}
afterEach(cleanup);

const row = (id: string, binding: Partial<UserDispatcher> = {}): UserDispatcher =>
	({
		id,
		name: `worker-${id}`,
		kind: 'docker',
		key_preview: 'abc…',
		last_seen_at: null,
		enabled: true,
		default_account: null,
		default_pool: null,
		...binding
	}) as unknown as UserDispatcher;

const click = (el: Element | null | undefined) => (el as HTMLElement).click();
const byText = (sel: string, text: string) =>
	[...document.querySelectorAll(sel)].find((e) => e.textContent?.trim() === text);

describe('DispatchersPanel renders a kit DataTable', () => {
	it('shows the DataTable loading row while the query is pending', () => {
		list.isLoading = true;
		list.data = undefined;
		comp = mount(DispatchersPanel, { target: document.body });
		const table = document.querySelector('[data-tsu="DataTable"]');
		expect(table?.getAttribute('aria-busy')).toBe('true');
		expect(document.querySelector('tr[data-part="loading"]')).not.toBeNull();
		expect(document.querySelector('table.disp, .placeholder')).toBeNull();
	});

	it('renders rows with a rowActions cell and the empty string when there are none', () => {
		list.isLoading = false;
		list.data = [row('a'), row('b')];
		comp = mount(DispatchersPanel, { target: document.body });
		const rows = document.querySelectorAll('tbody tr');
		expect(rows).toHaveLength(2);
		const actions = [...rows[0].querySelectorAll('td:last-child button')].map((b) => b.textContent?.trim());
		expect(actions).toEqual(['Rename', 'Remove']);
		cleanup();

		list.data = [];
		comp = mount(DispatchersPanel, { target: document.body });
		expect(document.body.textContent).toContain('No dispatchers enrolled yet.');
	});
});

describe('the default binding picker', () => {
	it('offers pools and accounts, preselects the bound pool, and saves it', async () => {
		list.isLoading = false;
		list.data = [row('a', { default_pool: 'work' } as Partial<UserDispatcher>)];
		pools.data = [{ id: 'p1', name: 'work' }];
		comp = mount(DispatchersPanel, { target: document.body });

		click(byText('td:last-child button', 'Rename'));
		await Promise.resolve();

		const groups = [...document.querySelectorAll('optgroup')].map((g) => g.label);
		expect(groups).toEqual(['Pools', 'Accounts']);
		const select = document.querySelector('[data-tsu="Modal"] select') as HTMLSelectElement;
		expect(select.value).toBe('pool\u0000work');

		click(byText('button', 'Save'));
		await Promise.resolve();
		expect(rename).toHaveBeenCalledWith('a', {
			name: 'worker-a',
			default_account: '',
			default_pool: 'work'
		});
	});
});
