// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import ResourceShares from './ResourceShares.svelte';

const grant = vi.fn(async () => ({}));
const share = (user_id: string, action: string) => ({
	resource_type: 'machine',
	resource_id: 'mc-1',
	user_id,
	user_name: `user-${user_id}`,
	action,
	granted_at: '2026-01-01T00:00:00Z'
});

vi.mock('$lib/paraglide/messages', () => ({
	m: new Proxy({}, { get: (_, key) => () => String(key) })
}));

vi.mock('$lib/queries', () => ({
	useResourceShares: () => ({ data: [share('a', 'use'), share('b', 'use'), share('b', 'spawn')] }),
	useResourceShareActions: () => ({ grant, revoke: vi.fn() })
}));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	grant.mockClear();
});

const actions = [
	{ value: 'use', label: 'Use (read only)' },
	{ value: 'spawn', label: 'Spawn sessions' }
];

async function shareWith(user: string) {
	const input = document.querySelector('input') as HTMLInputElement;
	input.value = user;
	input.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
	(document.querySelector('.share-add button') as HTMLButtonElement).click();
	await vi.waitFor(() => expect(grant).toHaveBeenCalled());
}

it('lists every grant a user holds, labelled by action', () => {
	comp = mount(ResourceShares, { target: document.body, props: { resourceType: 'machine', id: 'mc-1', actions } });
	const rows = [...document.querySelectorAll('.share-row')].map((r) => r.textContent ?? '');
	expect(rows).toHaveLength(3);
	expect(rows[2]).toContain('user-b');
	expect(rows[2]).toContain('Spawn sessions');
	expect(rows[0]).toContain('Use (read only)');
});

it('grants the picked action', async () => {
	comp = mount(ResourceShares, { target: document.body, props: { resourceType: 'machine', id: 'mc-1', actions } });
	const select = document.querySelector('select') as HTMLSelectElement;
	expect([...select.options].map((o) => o.value)).toEqual(['use', 'spawn']);
	select.value = 'spawn';
	select.dispatchEvent(new Event('change', { bubbles: true }));
	flushSync();
	await shareWith('carol');
	expect(grant).toHaveBeenCalledWith('machine', 'mc-1', { user: 'carol', action: 'spawn' });
});

it('defaults to the first action', async () => {
	comp = mount(ResourceShares, { target: document.body, props: { resourceType: 'machine', id: 'mc-1', actions } });
	await shareWith('dave');
	expect(grant).toHaveBeenCalledWith('machine', 'mc-1', { user: 'dave', action: 'use' });
});

it('offers no picker and sends no action for a single-action resource', async () => {
	comp = mount(ResourceShares, { target: document.body, props: { resourceType: 'account', id: 'acc-1' } });
	expect(document.querySelector('select')).toBeNull();
	await shareWith('erin');
	expect(grant).toHaveBeenCalledWith('account', 'acc-1', { user: 'erin' });
});
