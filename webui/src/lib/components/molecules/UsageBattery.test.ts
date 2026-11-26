// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const win = (key: string, utilization: number) => ({ key, kind: key, label: key, utilization });
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
			windows: [win('session', 20), win('weekly_all', 88)],
			age_secs: 0,
			limit_reset: null,
			limit_resets: []
		},
		{
			account: 'acct-2',
			account_name: 'qtax',
			account_emoji: null,
			header_pin: true,
			account_id: 'prov-2',
			provider: 'codex',
			usage: null,
			windows: [win('session', 40)],
			age_secs: 0,
			limit_reset: null,
			limit_resets: []
		}
	]
};
const accounts = {
	data: [{ id: 'acct-1', providers: [{ id: 'prov-1', soft_limits: { weekly_all: { cap_pct: 95 } } }] }]
};

vi.mock('#lib/queries/index.ts', () => ({
	useAllAccountsUsage: () => usage,
	useAccounts: () => accounts,
	usageKeys: { all: () => ['usage'], one: (id: string) => ['usage', id] }
}));
vi.mock('@tanstack/svelte-query', () => ({ useQueryClient: () => ({ setQueryData: vi.fn() }) }));
vi.mock('#lib/ws.svelte.ts', () => ({ ws: { onAccountUsage: () => () => {} } }));

import UsageBattery from './UsageBattery.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	accounts.data[0].providers[0].soft_limits.weekly_all.cap_pct = 95;
});

const render = () => {
	comp = mount(UsageBattery, { target: document.body });
	flushSync();
};

describe('the header usage gauge', () => {
	it('is one control that reads the worst window as text', () => {
		render();
		const gauges = document.querySelectorAll('.gauge');
		expect(gauges).toHaveLength(1);
		expect(gauges[0].textContent).toContain('7d 88%');
	});

	it('draws the worst window as a filled meter beside the text', () => {
		render();
		const meter = document.querySelector('.gauge .meter') as HTMLElement;
		expect(meter.dataset.tone).toBe('ok');
		expect((meter.querySelector('.fill') as HTMLElement).style.width).toBe('88%');
	});

	it('stays neutral below the soft limit and turns amber past it', () => {
		render();
		expect(document.querySelector('.gauge')?.getAttribute('data-tone')).toBe('ok');
		cleanupAndSetCap(50);
		expect(document.querySelector('.gauge')?.getAttribute('data-tone')).toBe('warn');
	});

	it('lists each account in the popover and links to /accounts', async () => {
		render();
		const panel = document.querySelector('[popover]') as HTMLElement;
		panel.dispatchEvent(Object.assign(new Event('toggle'), { newState: 'open' }));
		flushSync();
		await Promise.resolve();
		flushSync();
		const text = panel.textContent ?? '';
		expect(text).toContain('dev1');
		expect(text).toContain('qtax');
		const readings = [...panel.querySelectorAll('.reading')].map((r) => ({
			text: r.textContent?.replace(/\s+/g, ' ').trim(),
			fill: (r.querySelector('.meter .fill') as HTMLElement | null)?.style.width
		}));
		expect(readings).toContainEqual({ text: '5h 40%', fill: '40%' });
		expect(readings).toContainEqual({ text: '7d 88%', fill: '88%' });
		expect(panel.querySelector('a[href="/accounts"]')).not.toBeNull();
	});

	it('leaves the provider name to the icon', async () => {
		render();
		const panel = document.querySelector('[popover]') as HTMLElement;
		panel.dispatchEvent(Object.assign(new Event('toggle'), { newState: 'open' }));
		flushSync();
		await Promise.resolve();
		flushSync();
		const who = [...panel.querySelectorAll('.who')].map((w) => w.textContent?.trim());
		expect(who).toEqual(['dev1', 'qtax']);
	});
});

function cleanupAndSetCap(cap: number) {
	if (comp) unmount(comp);
	document.body.innerHTML = '';
	accounts.data[0].providers[0].soft_limits.weekly_all.cap_pct = cap;
	render();
}
