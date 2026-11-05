// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import type { SessionListItem } from '@bindings/SessionListItem';

const versionData = {
	version: '0.11.0',
	commit_url: 'https://example.test/commit',
	instance_name: null as string | null,
	latest_version: null as string | null,
	latest_url: null as string | null,
	repo_url: 'https://example.test',
	self_update_ready: false,
	self_update_hook: null
};
const sessionsData = { sessions: [] as SessionListItem[] };

const usageData: { data: unknown[] } = { data: [] };
const machinesData: { data: unknown[] } = { data: [] };
const { refreshVersion } = vi.hoisted(() => ({ refreshVersion: vi.fn() }));

vi.mock('$lib/queries', () => ({
	qk: {
		sessions: (a: boolean) => ['sessions', a],
		sessionsAll: ['sessions'],
		machineResources: ['machine-resources'],
		version: ['version']
	},
	usageKeys: { all: () => ['usage'], one: (id: string) => ['usage', id] },
	endpoints: { refreshVersion },
	useMe: () => ({ data: { user_name: 'dorsk', role: 'admin' } }),
	useVersion: () => ({ data: versionData }),
	useSessions: () => ({ data: sessionsData }),
	useAccounts: () => ({ data: [] }),
	useAllAccountsUsage: () => usageData,
	useProviderStatus: () => ({ data: [] }),
	useRedirectChips: () => ({ data: [] }),
	useMachineResources: () => machinesData,
	usePlugins: () => ({ data: [] })
}));
vi.mock('@tanstack/svelte-query', () => ({
	useQueryClient: () => ({ invalidateQueries: vi.fn(), setQueryData: vi.fn() })
}));
const { goto } = vi.hoisted(() => ({ goto: vi.fn() }));
vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$lib/ws.svelte', () => ({
	ws: {
		status: 'open',
		changeTick: 0,
		onListPatch: () => () => {},
		onMachineResources: () => () => {},
		onAccountUsage: () => () => {}
	}
}));
vi.mock('$lib/toast.svelte', () => ({
	toasts: { ok: vi.fn(), info: vi.fn(), error: vi.fn() }
}));

vi.stubGlobal('__CLIENT_VERSION__', '0.11.0');

import { notify } from '$lib/notify.svelte';
import Header from './Header.svelte';

let comp: ReturnType<typeof mount> | null = null;
function cleanup() {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
}
afterEach(cleanup);

beforeEach(() => {
	goto.mockClear();
	versionData.latest_version = null;
	versionData.version = '0.11.0';
	usageData.data = [];
	machinesData.data = [];
	refreshVersion.mockReset();
	sessionsData.sessions = [];
	notify.enabled = false;
});

const render = () => {
	comp = mount(Header, { target: document.body });
	flushSync();
};

async function openUserMenu() {
	const panel = [...document.querySelectorAll('[popover]')].at(-1) as HTMLElement;
	panel.dispatchEvent(Object.assign(new Event('toggle'), { newState: 'open' }));
	flushSync();
	await Promise.resolve();
	flushSync();
}

const menuRows = () =>
	[...document.querySelectorAll('[role="menuitem"], [role="menuitemcheckbox"]')] as HTMLElement[];
const rowByText = (text: string) =>
	menuRows().find((r) => r.textContent?.includes(text)) ?? null;

describe('the system bar carries neither the ? nor the bell', () => {
	it('has no standalone guides button in the tail', () => {
		render();
		const tail = document.querySelector('.tail') as HTMLElement;
		expect(tail.textContent).not.toContain('?');
		expect(tail.querySelector('a[href="/settings/guides"]')).toBeNull();
	});

	it('leaves Getting started to Settings › Guides, not the user menu', async () => {
		render();
		await openUserMenu();
		expect(rowByText('Getting started')).toBeNull();
	});

	it('offers the notification toggle in the menu and reflects its pressed state', async () => {
		render();
		await openUserMenu();
		const row = rowByText('Notify me when a session needs input');
		expect(row).not.toBeNull();
		expect(row?.getAttribute('role')).toBe('menuitemcheckbox');
		expect(row?.getAttribute('aria-checked')).toBe('false');
	});

	it('gives every menu row an icon', async () => {
		render();
		await openUserMenu();
		const rows = menuRows();
		expect(rows.length).toBeGreaterThan(0);
		for (const r of rows) expect(r.querySelector('svg')).not.toBeNull();
	});

	it('leaves Settings to the main nav, which is the one way in', async () => {
		render();
		await openUserMenu();
		expect(rowByText('Settings')).toBeNull();
	});
});

describe('notifications', () => {
	it('keep no indicator on the pill; the toggle lives in the user menu', async () => {
		notify.enabled = false;
		render();
		expect(document.querySelector('.pill .bell')).toBeNull();
		await openUserMenu();
		expect(rowByText('Notify me when a session needs input')).toBeTruthy();
	});
});

describe('the header carries no version unless it needs attention', () => {
	it('shows nothing when ui and server agree and no update is out', () => {
		render();
		expect(document.querySelector('.vers')).toBeNull();
		expect(document.querySelector('.lead')?.textContent).not.toContain('v0.11.0');
	});

	it('shows both versions when the ui and the server differ', () => {
		versionData.version = '0.12.0';
		render();
		const vers = document.querySelector('.vers') as HTMLElement;
		expect(vers.textContent).toContain('ui v0.11.0');
		expect(vers.textContent).toContain('srv v0.12.0');
	});

	it('shows an update chip and the avatar dot when an update is out', () => {
		versionData.latest_version = '0.12.0';
		render();
		expect(document.querySelector('.vers')?.textContent).toContain('0.12.0');
		expect(document.querySelector('.pill [data-tsu="Avatar"] [data-tsu="Dot"]')).not.toBeNull();
	});

	it('drops the alert dot when there is no update', () => {
		render();
		expect(document.querySelector('.pill [data-tsu="Avatar"] [data-tsu="Dot"]')).toBeNull();
	});
});

describe('the account menu', () => {
	it('carries the version with a check for updates', async () => {
		refreshVersion.mockResolvedValue({ ...versionData, latest_version: null });
		render();
		await openUserMenu();
		const row = rowByText('Version 0.11.0');
		expect(row?.textContent).toContain('Check for updates');
		row?.click();
		await Promise.resolve();
		expect(refreshVersion).toHaveBeenCalledOnce();
	});

	it('offers Appearance, and the header no longer holds the theme or text-size pickers', async () => {
		render();
		const tail = document.querySelector('.tail') as HTMLElement;
		expect(tail.querySelector('[data-tsu="ThemePicker"]')).toBeNull();
		await openUserMenu();
		expect(rowByText('Appearance')).not.toBeNull();
	});

	it('links admins to the events page', async () => {
		render();
		await openUserMenu();
		rowByText('Events')?.click();
		expect(goto).toHaveBeenCalledWith('/events');
	});
});

describe('the status button', () => {
	const usageRow = (utilization: number) => ({
		account: 'a1',
		account_name: 'dev1',
		account_emoji: null,
		header_pin: true,
		account_id: 'p1',
		provider: 'anthropic',
		usage: null,
		windows: [{ key: 'weekly_all', kind: 'weekly_all', label: 'Weekly', utilization }],
		age_secs: 0,
		limit_reset: null
	});

	it('is absent when there is nothing to report', () => {
		render();
		expect(document.querySelector('.status')).toBeNull();
	});

	it('reads the worst window as text, neutral below the limit', () => {
		usageData.data = [usageRow(42.4)];
		render();
		const status = document.querySelector('.status') as HTMLElement;
		expect(status.textContent).toContain('7d 42%');
		expect(status.dataset.tone).toBe('ok');
	});

	it('turns amber near the wall', () => {
		usageData.data = [usageRow(93)];
		render();
		expect((document.querySelector('.status') as HTMLElement).dataset.tone).toBe('warn');
	});
});

describe('the user pill avatar', () => {
	it('carries the user initial and stays out of the accessibility tree', () => {
		render();
		const av = document.querySelector('.pill [data-tsu="Avatar"]') as HTMLElement;
		expect(av.textContent?.trim()).toBe('D');
		expect(av.getAttribute('aria-hidden')).toBe('true');
		expect(av.getAttribute('role')).toBeNull();
	});
});
