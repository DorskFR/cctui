import { describe, expect, it } from 'vitest';
import { QueryClient } from '@tanstack/svelte-query';
import type { AccountUsageEntry } from '@bindings/AccountUsageEntry';
import { bindAccountUsageLive, usageKey } from './data.svelte';
import type { PushedUsage } from './model';

const entry = (id: string, utilization: number): AccountUsageEntry => ({
	account: 'acct',
	account_name: 'personal',
	account_emoji: null,
	header_pin: true,
	account_id: id,
	provider: 'anthropic',
	usage: null,
	windows: [{ key: 'session', kind: 'session', label: '5h', utilization }],
	age_secs: 0,
	limit_reset: null,
	limit_resets: []
});

function source() {
	const cbs = new Set<(ev: { account_id: string; usage: PushedUsage }) => void>();
	return {
		onAccountUsage(cb: (ev: { account_id: string; usage: PushedUsage }) => void) {
			cbs.add(cb);
			return () => cbs.delete(cb);
		},
		push(id: string, utilization: number) {
			const { account_id, provider, usage, windows, age_secs, limit_reset } = entry(id, utilization);
			for (const cb of cbs) cb({ account_id: id, usage: { account_id, provider, usage, windows, age_secs, limit_reset } });
		},
		get size() {
			return cbs.size;
		}
	};
}

describe('bindAccountUsageLive', () => {
	it('patches the one batch cache from a push and unbinds cleanly', () => {
		const qc = new QueryClient();
		qc.setQueryData<AccountUsageEntry[]>(usageKey, [entry('c1', 10), entry('c2', 20)]);
		const src = source();
		const off = bindAccountUsageLive(qc, src);

		src.push('c1', 72);
		const rows = qc.getQueryData<AccountUsageEntry[]>(usageKey);
		expect(rows?.[0].windows[0].utilization).toBe(72);
		expect(rows?.[1].windows[0].utilization).toBe(20);

		off();
		expect(src.size).toBe(0);
	});

	it('leaves the cache untouched for a credential it does not hold', () => {
		const qc = new QueryClient();
		const before = [entry('c1', 10)];
		qc.setQueryData<AccountUsageEntry[]>(usageKey, before);
		const src = source();
		bindAccountUsageLive(qc, src);
		src.push('other', 99);
		expect(qc.getQueryData(usageKey)).toBe(before);
	});
});
