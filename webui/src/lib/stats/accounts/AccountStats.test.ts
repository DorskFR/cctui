// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { AccountLedgerRow } from '@bindings/AccountLedgerRow';

const row = (over: Partial<AccountLedgerRow>): AccountLedgerRow => ({
	account_id: 'c1',
	account_name: 'personal',
	account_emoji: null,
	provider: 'anthropic',
	window_key: 'weekly_all',
	window_label: 'Weekly',
	resets_at: new Date().toISOString(),
	final_utilization: 94,
	unused_pct: 6,
	unused_capacity_hours: 10,
	amount_usd: null,
	sampling_gap: false,
	idle: false,
	...over
});

const ledger = {
	rows: [
		row({}),
		row({ account_id: 'c2', account_name: 'work', idle: true, final_utilization: 0, unused_pct: 100 })
	],
	totals: [
		{
			account_id: 'c1',
			account_name: 'personal',
			window_key: 'weekly_all',
			closes: 1,
			unused_pct_mean: 6,
			unused_capacity_hours: 10,
			idle_closes: 0
		}
	]
};
const usage = [
	{
		account: 'a1',
		account_name: 'personal',
		account_emoji: null,
		header_pin: true,
		account_id: 'c1',
		provider: 'anthropic',
		usage: null,
		windows: [
			{ key: 'session', kind: 'session', label: '5h', utilization: 42, resets_at: null },
			{ key: 'weekly_all', kind: 'weekly_all', label: 'Weekly', utilization: 61, resets_at: null }
		],
		age_secs: 0,
		limit_reset: null,
		limit_resets: []
	}
];

vi.mock('./data.svelte', () => ({
	useAccountLedger: () => ({ data: ledger, isLoading: false, isError: false }),
	useAllAccountsUsage: () => ({ data: usage, isLoading: false })
}));

import AccountStats from './AccountStats.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function render(props: Record<string, unknown>) {
	const host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(AccountStats, { target: host, props });
	flushSync();
	return host;
}

describe('AccountStats', () => {
	it('mini lists one line per credential, idle ones as idle', () => {
		const host = render({ size: 'mini' });
		const text = host.textContent ?? '';
		expect(text).toMatch(/personal: lost 6% weekly at/);
		expect(text).toContain('work: idle');
		expect(host.querySelector('table')).toBeNull();
	});

	it('mini narrows to one credential', () => {
		const host = render({ size: 'mini', account: 'c2' });
		expect(host.textContent).toContain('work: idle');
		expect(host.textContent).not.toContain('personal');
	});

	it('light adds the current meters to the one-liners', () => {
		const host = render({ size: 'light' });
		expect(host.querySelectorAll('[role="progressbar"]').length).toBe(2);
		expect(host.textContent).toContain('work: idle');
	});

	it('full shows current windows, totals and the ledger with idle rows marked idle', () => {
		const host = render({ size: 'full' });
		const text = host.textContent ?? '';
		expect(text).toContain('Current windows');
		expect(text).toContain('42%');
		expect(text).toContain('weekly: 1 resets, 0 idle, 6% lost on average when used');
		expect(text).toContain('Quota resets, last 7 days');
		expect(text).toContain('idle');
		expect(text).not.toContain('100%');
	});
});
