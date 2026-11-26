// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Notification as Notice } from '@bindings/Notification';
import type { NotificationList } from '@bindings/NotificationList';
import { NotificationStore, type NotificationApi } from '#lib/notifications.svelte.ts';
import { ApiError } from '#lib/api.ts';
import { notify } from '#lib/notify.svelte.ts';
import { settings } from '#lib/settings.svelte.ts';

const { toasts } = vi.hoisted(() => ({
	toasts: { ok: vi.fn(), info: vi.fn(), error: vi.fn() }
}));
vi.mock('#lib/toast.svelte.ts', () => ({ toasts }));
const { goto } = vi.hoisted(() => ({ goto: vi.fn(async () => {}) }));
vi.mock('$app/navigation', () => ({ goto }));

import NotificationBell from './NotificationBell.svelte';
import { m } from '#lib/paraglide/messages.js';

const dialogButton = (label: string) =>
	[...document.querySelectorAll<HTMLButtonElement>('dialog[open] button')].find(
		(b) => b.textContent?.trim() === label
	);

const NOW = new Date().toISOString();

function notice(id: string, over: Partial<Notice> = {}): Notice {
	return {
		id,
		user_id: 'u-1',
		kind: 'approval',
		severity: 'high',
		session_id: null,
		machine_id: null,
		subject_kind: null,
		subject_id: null,
		collapse_key: null,
		title: `Release ${id}?`,
		body: 'ship it',
		tags: null,
		actions: [{ id: 'approve', label: 'Approve' }],
		state: 'delivered',
		repeat_count: 1,
		decision: null,
		decision_note: null,
		acted_channel: null,
		acted_by: null,
		resolved_reason: null,
		created_at: NOW,
		updated_at: NOW,
		read_at: null,
		acted_at: null,
		resolved_at: null,
		expires_at: null,
		...over
	};
}

let comp: ReturnType<typeof mount> | null = null;
afterEach(async () => {
	if (comp) await unmount(comp);
	comp = null;
	document.body.replaceChildren();
	vi.clearAllMocks();
	vi.unstubAllGlobals();
});

function setup(
	api: Partial<NotificationApi>,
	extra: Notice[] = [],
	redispatch: (record: string) => Promise<unknown> = vi.fn(async () => ({}))
) {
	const store: NotificationStore = new NotificationStore({
		list: vi.fn(async (): Promise<NotificationList> => ({
			notifications: store.items,
			unread: store.unread,
			has_more: false
		})),
		read: vi.fn(),
		act: vi.fn(),
		dismiss: vi.fn(),
		readAll: vi.fn(),
		...api
	});
	store.upsert(notice('a'));
	store.upsert(notice('b', { state: 'read' }));
	for (const n of extra) store.upsert(n);
	store.setUnread(1);
	comp = mount(NotificationBell, { target: document.body, props: { store, redispatch } });
	flushSync();
	return store;
}

async function openPanel() {
	const panel = document.querySelector('[popover]') as HTMLElement;
	panel.dispatchEvent(Object.assign(new Event('toggle'), { newState: 'open' }));
	flushSync();
	await tick();
	flushSync();
}

const dismissButton = (id: string) =>
	document.querySelector(`[data-id="${id}"] .notif-dismiss button`) as HTMLElement;

const resolved = (id: string, decision: string) => ({
	notification: notice(id, { state: decision === 'expired' ? 'expired' : 'resolved', decision }),
	applied: true
});

const trigger = () => document.querySelector('[aria-haspopup="dialog"]') as HTMLElement;

describe('NotificationBell', () => {
	it('badges the server unread count on the bell', () => {
		const store = setup({});
		expect(trigger().textContent).toContain('1');
		store.setUnread(4);
		flushSync();
		expect(trigger().textContent).toContain('4');
	});

	it('lists the open notices once opened', async () => {
		setup({});
		await openPanel();
		expect(document.body.textContent).toContain('Release a?');
		expect(document.body.textContent).toContain('Release b?');
	});

	it('answers from the web and says so when another channel won', async () => {
		const won = notice('a', { state: 'acted', decision: 'approve', acted_channel: 'ntfy' });
		const act = vi.fn(async () => ({ notification: won, applied: false }));
		setup({ act });
		await openPanel();
		const button = document.querySelector('[data-id="a"] [data-action="approve"]') as HTMLElement;
		button.click();
		await vi.waitFor(() => expect(toasts.info).toHaveBeenCalled());
		expect(act).toHaveBeenCalledWith('a', { action: 'approve', note: null, channel: 'web' });
		expect(toasts.info.mock.calls[0][0]).toContain('approve via ntfy');
		expect(toasts.error).not.toHaveBeenCalled();
	});

	it('mark all read clears the badge and keeps an approval answerable', async () => {
		const readAll = vi.fn(async () => ({ read: 1, unread: 0 }));
		const list = vi
			.fn<NotificationApi['list']>()
			.mockResolvedValueOnce({
				notifications: [notice('a'), notice('b', { state: 'read' })],
				unread: 1,
				has_more: false
			})
			.mockResolvedValue({
				notifications: [notice('a', { state: 'read' }), notice('b', { state: 'read' })],
				unread: 0,
				has_more: false
			});
		setup({ readAll, list });
		await openPanel();
		await vi.waitFor(() => expect(list).toHaveBeenCalledTimes(1));
		expect(trigger().textContent).toContain('1');
		expect(document.querySelector('[data-id="a"]')?.hasAttribute('data-unread')).toBe(true);
		const markAll = [...document.querySelectorAll('button')].find(
			(b) => b.textContent?.trim() === 'Mark all read'
		) as HTMLButtonElement;
		markAll.click();
		await vi.waitFor(() => expect(list).toHaveBeenCalledTimes(2));
		flushSync();
		expect(readAll).toHaveBeenCalledOnce();
		expect(trigger().textContent).not.toContain('1');
		const row = document.querySelector('[data-id="a"]') as HTMLElement;
		expect(row.hasAttribute('data-unread')).toBe(false);
		expect(row.hasAttribute('data-resolved')).toBe(false);
		expect(row.textContent).toContain('Needs an answer');
		expect(row.querySelector('[data-action="approve"]')).not.toBeNull();
		expect(toasts.error).not.toHaveBeenCalled();
	});

	it('says so when mark all read fails', async () => {
		setup({ readAll: vi.fn(async () => Promise.reject(new Error('offline'))) });
		await openPanel();
		const markAll = [...document.querySelectorAll('button')].find(
			(b) => b.textContent?.trim() === 'Mark all read'
		) as HTMLButtonElement;
		markAll.click();
		await vi.waitFor(() => expect(toasts.error).toHaveBeenCalled());
		expect(toasts.error.mock.calls[0][0]).toContain('offline');
	});

	it('shows an open link only on a notice with a target, anchored to its message', async () => {
		const anchored = notice('t', { session_id: 's-7', anchor_seq: 42, actions: [] });
		setup({}, [anchored]);
		await openPanel();
		const link = document.querySelector('[data-id="t"] a.notif-title') as HTMLAnchorElement;
		expect(link.getAttribute('href')).toBe('/sessions?session=s-7&seq=42');
		expect(document.querySelector('[data-id="a"] a.notif-title'), 'no target, no link').toBeNull();
	});

	it('re-dispatches a failed dispatch from its notice, not through the notice API', async () => {
		const failed = notice('f', {
			kind: 'dispatch_failed',
			severity: 'critical',
			subject_kind: 'dispatch',
			subject_id: 'd-1',
			actions: []
		});
		const act = vi.fn();
		const redispatch = vi.fn(async () => ({}));
		setup({ act }, [failed], redispatch);
		await openPanel();
		const link = document.querySelector('[data-id="f"] a.notif-title') as HTMLAnchorElement;
		expect(link.getAttribute('href')).toBe('/dispatch-failures?record=d-1');
		(document.querySelector('[data-id="f"] [data-action="redispatch"]') as HTMLElement).click();
		await vi.waitFor(() => expect(toasts.ok).toHaveBeenCalled());
		expect(redispatch).toHaveBeenCalledWith('d-1');
		expect(act).not.toHaveBeenCalled();

		const details = () =>
			document.querySelector('[data-id="f"] [data-action="details"]') as HTMLButtonElement;
		await vi.waitFor(() => {
			flushSync();
			expect(details().disabled).toBe(false);
		});
		details().click();
		await vi.waitFor(() => expect(goto).toHaveBeenCalledWith('/dispatch-failures?record=d-1'));
		expect(act).not.toHaveBeenCalled();
	});

	it('dismisses a notice nobody waits on without asking', async () => {
		const dismiss = vi.fn(async () => resolved('a', 'dismissed'));
		const store = setup({ dismiss });
		await openPanel();
		dismissButton('a').click();
		await vi.waitFor(() => expect(store.items.map((n) => n.id)).not.toContain('a'));
		expect(dismiss).toHaveBeenCalledTimes(1);
		expect(dismiss).toHaveBeenCalledWith('a', false);
		expect(document.querySelector('dialog[open] [data-tsu="ConfirmModal"]')).toBeNull();
		expect(toasts.error).not.toHaveBeenCalled();
	});

	it('asks before expiring a notice a session still waits on', async () => {
		const dismiss = vi.fn(async (id: string, confirm?: boolean) => {
			if (!confirm) throw new ApiError(409, 'a run is waiting on this');
			return resolved(id, 'expired');
		});
		const store = setup({ dismiss });
		await openPanel();
		dismissButton('a').click();
		await vi.waitFor(() => expect(dialogButton(m.notif_dismiss_anyway())).toBeTruthy());
		expect(dismiss).toHaveBeenCalledTimes(1);
		dialogButton(m.notif_dismiss_anyway())?.click();
		await vi.waitFor(() => expect(dismiss).toHaveBeenCalledTimes(2));
		expect(dismiss.mock.calls.map((c) => c[1])).toEqual([false, true]);
		await vi.waitFor(() => expect(store.items.map((n) => n.id)).not.toContain('a'));
		expect(toasts.error).not.toHaveBeenCalled();
		expect(toasts.info).not.toHaveBeenCalled();
	});

	it('keeps a waited-on notice when the confirm is declined', async () => {
		const dismiss = vi.fn(async () => Promise.reject(new ApiError(409, 'waiting')));
		const store = setup({ dismiss });
		await openPanel();
		dismissButton('a').click();
		await vi.waitFor(() => expect(dialogButton(m.common_cancel())).toBeTruthy());
		dialogButton(m.common_cancel())?.click();
		await tick();
		expect(dismiss).toHaveBeenCalledTimes(1);
		expect(store.items.map((n) => n.id)).toContain('a');
		expect(toasts.error).not.toHaveBeenCalled();
	});
});

describe('desktop notifications switch', () => {
	const toggle = () =>
		document.querySelector('[data-tsu="NotificationCenter"] [role="switch"]') as HTMLButtonElement | null;

	afterEach(async () => {
		await settings.setNotifyEnabled(false);
	});

	it('sits in the open panel, reflecting the setting', async () => {
		setup({});
		await openPanel();
		const sw = toggle();
		expect(sw).not.toBeNull();
		expect(document.querySelector('[data-tsu="NotificationCenter"]')?.textContent).toContain(m.notif_desktop_toggle());
		expect(sw?.getAttribute('aria-checked')).toBe('false');
	});

	it('turns desktop notifications on once the browser grants them', async () => {
		vi.stubGlobal('Notification', { permission: 'default', requestPermission: vi.fn(async () => 'granted') });
		setup({});
		await openPanel();
		toggle()?.click();
		await vi.waitFor(() => expect(settings.notifyEnabled).toBe(true));
		flushSync();
		expect(toggle()?.getAttribute('aria-checked')).toBe('true');
		expect(toasts.ok).toHaveBeenCalledWith(m.nav_notify_on());
	});

	it('stays off when the browser blocks them', async () => {
		vi.stubGlobal('Notification', { permission: 'default', requestPermission: vi.fn(async () => 'denied') });
		setup({});
		await openPanel();
		toggle()?.click();
		await vi.waitFor(() => expect(toasts.error).toHaveBeenCalledWith(m.nav_notify_blocked()));
		flushSync();
		expect(settings.notifyEnabled).toBe(false);
		expect(toggle()?.getAttribute('aria-checked')).toBe('false');
	});

	it('mutes them when on', async () => {
		vi.stubGlobal('Notification', { permission: 'granted', requestPermission: vi.fn(async () => 'granted') });
		await settings.setNotifyEnabled(true);
		expect(notify.enabled).toBe(true);
		setup({});
		await openPanel();
		expect(toggle()?.getAttribute('aria-checked')).toBe('true');
		toggle()?.click();
		await vi.waitFor(() => expect(settings.notifyEnabled).toBe(false));
		flushSync();
		expect(toggle()?.getAttribute('aria-checked')).toBe('false');
		expect(toasts.info).toHaveBeenCalledWith(m.nav_notify_off());
	});
});
