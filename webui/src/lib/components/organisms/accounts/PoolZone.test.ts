import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import type { AccountPoolView } from '@bindings/AccountPoolView';
import type { PoolUsageView } from '@bindings/PoolUsageView';
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

describe('PoolZone gauges', () => {
	const usage = (failover: boolean): PoolUsageView => ({
		pool_id: 'p1',
		name: 'production',
		strategy: 'headroom',
		failover,
		families: [
			{
				family: 'anthropic',
				members: [{ account_id: 'a0', name: 'one', emoji: null, weight: 1, usage_known: true }],
				windows: [
					{
						key: 'weekly_all',
						kind: 'weekly_all',
						label: 'Weekly (all models)',
						model_display_name: null,
						level_pct: 43,
						expected_pct: 31,
						ratio: 1.38,
						next_reset_at: new Date(Date.now() + 86_400_000).toISOString(),
						members: [],
						projection: null,
						projection_unavailable: 'insufficient_history'
					}
				]
			}
		]
	});

	it('renders no gauge block without usage', () => {
		comp = mount(PoolZone, { target: document.body, props: { pool: pool(1), accounts: [] } });
		expect(document.querySelector('[data-journey="pool-usage"]')).toBeNull();
	});

	it('renders the aggregate above the members when usage is known', () => {
		comp = mount(PoolZone, {
			target: document.body,
			props: { pool: pool(1), accounts: [], usage: usage(false) }
		});
		const block = document.querySelector('[data-journey="pool-usage"]');
		expect(block).not.toBeNull();
		expect(block?.textContent).toContain('43%');
		expect(block?.textContent).toContain('Failover off');
	});
});
