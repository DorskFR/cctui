// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const usage = {
	data: [
		{
			account: 'acct-1',
			account_name: 'dev1',
			account_emoji: null,
			header_pin: true,
			account_id: 'prov-1',
			provider: 'claude',
			usage: null,
			windows: [{ key: 'weekly_all', kind: 'weekly_all', label: 'weekly', utilization: 88 }],
			age_secs: 0,
			limit_reset: null,
			limit_resets: []
		}
	]
};
const machines = {
	data: [
		{
			machine_id: 'm1',
			name: 'box',
			display_name: null,
			liveness: 'online',
			updated_at: new Date().toISOString(),
			resources: { cpu_pct: 95, mem_pct: 10, disk_pct: 10 }
		}
	]
};

vi.mock('#lib/queries/index.ts', () => ({
	useAllAccountsUsage: () => usage,
	useAccounts: () => ({ data: [] }),
	useMachineResources: () => machines
}));
vi.mock('#lib/settings.svelte.ts', () => ({ settings: { monitoredMachines: ['m1'] } }));

import HeaderStatus from './HeaderStatus.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

describe('the mobile status button', () => {
	it('folds machines and usage into one control toned by the worst of them', () => {
		comp = mount(HeaderStatus, { target: document.body });
		flushSync();
		const buttons = document.querySelectorAll('.status');
		expect(buttons).toHaveLength(1);
		expect(buttons[0].textContent).toContain('7d 88%');
		expect(buttons[0].getAttribute('data-tone')).toBe('danger');
	});

	it('lists both usage and machines in its panel', async () => {
		comp = mount(HeaderStatus, { target: document.body });
		flushSync();
		const panel = document.querySelector('[popover]') as HTMLElement;
		panel.dispatchEvent(Object.assign(new Event('toggle'), { newState: 'open' }));
		flushSync();
		await Promise.resolve();
		flushSync();
		expect(panel.textContent).toContain('dev1');
		expect(panel.textContent).toContain('box');
		expect(panel.textContent).toContain('95%');
		expect(panel.querySelector('a[href="/accounts"]')).not.toBeNull();
	});
});
