// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Notification as Notice } from '@bindings/Notification';
import type { NotificationList } from '@bindings/NotificationList';
import { NotificationStore, type NotificationApi } from '#lib/notifications.svelte.ts';

const { toasts } = vi.hoisted(() => ({
	toasts: { ok: vi.fn(), info: vi.fn(), error: vi.fn() }
}));
vi.mock('#lib/toast.svelte.ts', () => ({ toasts }));
const { goto } = vi.hoisted(() => ({ goto: vi.fn(async () => {}) }));
vi.mock('$app/navigation', () => ({ goto }));

import NotificationBell from './NotificationBell.svelte';

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
});
