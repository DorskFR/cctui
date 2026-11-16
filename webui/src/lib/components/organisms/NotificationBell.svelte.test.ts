// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Notification as Notice } from '@bindings/Notification';
import { NotificationStore, type NotificationApi } from '$lib/notifications.svelte';

const { toasts } = vi.hoisted(() => ({
	toasts: { ok: vi.fn(), info: vi.fn(), error: vi.fn() }
}));
vi.mock('$lib/toast.svelte', () => ({ toasts }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

import NotificationBell from './NotificationBell.svelte';

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
		created_at: new Date().toISOString(),
		updated_at: new Date().toISOString(),
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

function setup(api: Partial<NotificationApi>) {
	const store = new NotificationStore({
		list: vi.fn(async () => ({
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
	store.setUnread(1);
	comp = mount(NotificationBell, { target: document.body, props: { store } });
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
		const button = document.querySelector('[data-action="approve"]') as HTMLElement;
		button.click();
		await vi.waitFor(() => expect(toasts.info).toHaveBeenCalled());
		expect(act).toHaveBeenCalledWith('a', { action: 'approve', note: null, channel: 'web' });
		expect(toasts.info.mock.calls[0][0]).toContain('approve via ntfy');
		expect(toasts.error).not.toHaveBeenCalled();
	});
});
