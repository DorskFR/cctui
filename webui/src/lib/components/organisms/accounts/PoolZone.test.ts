import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import type { AccountPoolView } from '@bindings/AccountPoolView';
import PoolZone from './PoolZone.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const pool = (n: number, failover = false): AccountPoolView =>
	({
		id: 'p1',
		user_id: 'u1',
		name: 'production',
		strategy: 'headroom',
		failover,
		created_at: '2026-01-01T00:00:00Z',
		members: Array.from({ length: n }, (_, i) => ({ account_id: `a${i}`, position: i }))
	}) as unknown as AccountPoolView;

const legend = (n: number, failover = false) => {
	comp = mount(PoolZone, { target: document.body, props: { pool: pool(n, failover), accounts: [] } });
	return document.querySelector('legend')?.textContent?.replace(/\s+/g, ' ').trim() ?? '';
};

describe('PoolZone legend counts its members', () => {
	it('says "0 accounts" when the pool is empty', () => {
		expect(legend(0)).toContain('pool · 0 accounts');
	});

	it('says "1 account", not "1 accounts", for a single member', () => {
		const text = legend(1);
		expect(text).toContain('pool · 1 account');
		expect(text).not.toContain('1 accounts');
	});

	it('says "2 accounts" for two members', () => {
		expect(legend(2)).toContain('pool · 2 accounts');
	});

	it('pluralises the failover variant for a single member', () => {
		expect(legend(1, true)).toContain('pool · 1 account · failover armed');
	});

	it('pluralises the failover variant for two members', () => {
		expect(legend(2, true)).toContain('pool · 2 accounts · failover armed');
	});
});
