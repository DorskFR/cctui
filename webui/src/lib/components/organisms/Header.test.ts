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

vi.mock('#lib/queries/index.ts', () => ({
	qk: { sessions: (a: boolean) => ['sessions', a], sessionsAll: ['sessions'] },
	useMe: () => ({ data: { user_name: 'dorsk', role: 'admin' } }),
	useVersion: () => ({ data: versionData }),
	useSessions: () => ({ data: sessionsData }),
	useAccounts: () => ({ data: [] }),
	useAllAccountsUsage: () => ({ data: [] }),
	useProviderStatus: () => ({ data: [] }),
	useRedirectChips: () => ({ data: [] }),
	useMachineResources: () => ({ data: [] }),
	usePlugins: () => ({ data: [] })
}));
vi.mock('@tanstack/svelte-query', () => ({
	useQueryClient: () => ({ invalidateQueries: vi.fn(), setQueryData: vi.fn() })
}));
const { goto } = vi.hoisted(() => ({ goto: vi.fn() }));
vi.mock('$app/navigation', () => ({ goto }));
vi.mock('#lib/ws.svelte.ts', () => ({
	ws: {
		status: 'open',
		changeTick: 0,
		onListPatch: () => () => {},
		onMachineResources: () => () => {},
		onAccountUsage: () => () => {},
		onNotification: () => () => {},
		onNotificationsUnread: () => () => {}
	}
}));
vi.mock('#lib/toast.svelte.ts', () => ({
	toasts: { ok: vi.fn(), info: vi.fn(), error: vi.fn() }
}));

vi.stubGlobal('__CLIENT_VERSION__', '0.11.0');

import { notify } from '#lib/notify.svelte.ts';
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

describe('the system bar carries no ? button', () => {
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

describe('versions stay out of the bar', () => {
	it('prints no ui/srv version in the header', () => {
		render();
		const hd = document.querySelector('header.hd') as HTMLElement;
		expect(hd.querySelector('.vers')).toBeNull();
		expect(hd.textContent).not.toContain('srv v');
		expect(hd.querySelector('.reload')).toBeNull();
	});

	it('shows a reload chip only when the ui and server builds differ', () => {
		versionData.version = '0.12.0';
		render();
		const chip = document.querySelector('header.hd .reload') as HTMLElement;
		expect(chip).not.toBeNull();
		expect(chip.title).toContain('ui v0.11.0 · srv v0.12.0');
	});

	it('keeps the version in the account menu', async () => {
		render();
		await openUserMenu();
		expect(rowByText('v0.11.0')).not.toBeNull();
	});

	it('leaves an available update to the avatar dot', () => {
		versionData.latest_version = '0.12.0';
		render();
		expect(document.querySelector('.pill [data-tsu="Avatar"] [data-tsu="Dot"]')).not.toBeNull();
	});

	it('drops the alert dot when there is no update', () => {
		render();
		expect(document.querySelector('.pill [data-tsu="Avatar"] [data-tsu="Dot"]')).toBeNull();
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
