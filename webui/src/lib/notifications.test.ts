import { describe, expect, it, vi } from 'vitest';
import type { Notification as Notice } from '@bindings/Notification';
import { ApiError } from './api';
import {
	NotificationStore,
	needsAnswer,
	noticeHref,
	toItem,
	type NotificationApi
} from './notifications.svelte';

function notice(id: string, over: Partial<Notice> = {}): Notice {
	return {
		id,
		user_id: 'u-1',
		kind: 'approval',
		severity: 'high',
		session_id: 's-1',
		machine_id: null,
		subject_kind: null,
		subject_id: null,
		collapse_key: null,
		title: `notice ${id}`,
		body: 'ship it',
		tags: null,
		actions: [
			{ id: 'approve', label: 'Approve' },
			{ id: 'reject', label: 'Reject', style: 'danger' }
		],
		state: 'delivered',
		repeat_count: 1,
		decision: null,
		decision_note: null,
		acted_channel: null,
		acted_by: null,
		resolved_reason: null,
		created_at: `2026-11-16T10:00:0${id.slice(-1)}Z`,
		updated_at: '2026-11-16T10:00:00Z',
		read_at: null,
		acted_at: null,
		resolved_at: null,
		expires_at: null,
		...over
	};
}

function fakeApi(over: Partial<NotificationApi> = {}): NotificationApi {
	return {
		list: vi.fn(async () => ({ notifications: [], unread: 0, has_more: false })),
		read: vi.fn(async (id: string) => ({
			notification: notice(id, { state: 'read' }),
			applied: true
		})),
		act: vi.fn(),
		dismiss: vi.fn(),
		readAll: vi.fn(async () => ({ read: 0, unread: 0 })),
		...over
	};
}

function fakeSource() {
	const noticeCbs = new Set<(n: Notice) => void>();
	const unreadCbs = new Set<(c: number) => void>();
	return {
		onNotification(cb: (n: Notice) => void) {
			noticeCbs.add(cb);
			return () => noticeCbs.delete(cb);
		},
		onNotificationsUnread(cb: (c: number) => void) {
			unreadCbs.add(cb);
			return () => unreadCbs.delete(cb);
		},
		frame: (n: Notice) => {
			for (const cb of noticeCbs) cb(n);
		},
		unread: (c: number) => {
			for (const cb of unreadCbs) cb(c);
		},
		listeners: () => noticeCbs.size + unreadCbs.size
	};
}

const ids = (store: NotificationStore) => store.items.map((n) => n.id);

describe('notification store', () => {
	it('upserts WS frames newest first and replaces by id', () => {
		const store = new NotificationStore(fakeApi());
		const ws = fakeSource();
		const off = store.connect(ws);
		ws.frame(notice('n1'));
		ws.frame(notice('n2'));
		expect(ids(store)).toEqual(['n2', 'n1']);
		ws.frame(notice('n1', { title: 'collapsed', repeat_count: 2 }));
		expect(ids(store)).toEqual(['n2', 'n1']);
		expect(store.items[1].title).toBe('collapsed');
		off();
		expect(ws.listeners()).toBe(0);
	});

	it('orders notices with the same timestamp by id, whatever the arrival order', () => {
		const at = '2026-11-16T10:00:00Z';
		const store = new NotificationStore(fakeApi());
		store.upsert(notice('b', { created_at: at }));
		store.upsert(notice('c', { created_at: at }));
		store.upsert(notice('a', { created_at: at }));
		expect(ids(store)).toEqual(['a', 'b', 'c']);
	});

	it('drops the card on a terminal state but keeps an acted one', () => {
		const store = new NotificationStore(fakeApi());
		const ws = fakeSource();
		store.connect(ws);
		ws.frame(notice('n1'));
		ws.frame(notice('n2'));
		ws.frame(notice('n1', { state: 'acted', decision: 'approve' }));
		expect(ids(store)).toEqual(['n2', 'n1']);
		ws.frame(notice('n1', { state: 'resolved' }));
		ws.frame(notice('n2', { state: 'expired' }));
		expect(store.items).toEqual([]);
	});

	it('syncs the unread count from frames and from the list', async () => {
		const list = vi.fn(async () => ({
			notifications: [notice('n1'), notice('n2', { state: 'resolved' })],
			unread: 7,
			has_more: true
		}));
		const store = new NotificationStore(fakeApi({ list }));
		const ws = fakeSource();
		store.connect(ws);
		ws.unread(3);
		expect(store.unread).toBe(3);
		await store.load();
		expect(store.unread).toBe(7);
		expect(ids(store)).toEqual(['n1']);
		expect(store.hasMore).toBe(true);
		ws.unread(-1);
		expect(store.unread).toBe(0);
	});

	it('reports a lost race as answered elsewhere, not as an error', async () => {
		const won = notice('n1', { state: 'acted', decision: 'reject', acted_channel: 'ntfy' });
		const act = vi.fn(async () => ({ notification: won, applied: false }));
		const store = new NotificationStore(fakeApi({ act }));
		store.upsert(notice('n1'));
		const out = await store.act('n1', 'approve');
		expect(act).toHaveBeenCalledWith('n1', { action: 'approve', note: null, channel: 'web' });
		expect(out.kind).toBe('elsewhere');
		expect(store.items[0].state).toBe('acted');
		expect(toItem(store.items[0]).state).toBe('reject via ntfy');
		expect(toItem(store.items[0]).actions).toEqual([]);
	});

	it('returns applied and failed outcomes', async () => {
		const ok = new NotificationStore(
			fakeApi({
				act: vi.fn(async () => ({
					notification: notice('n1', { state: 'acted', decision: 'approve' }),
					applied: true
				}))
			})
		);
		expect((await ok.act('n1', 'approve')).kind).toBe('applied');
		const broken = new NotificationStore(
			fakeApi({ act: vi.fn(async () => Promise.reject(new ApiError(500, 'boom'))) })
		);
		expect((await broken.act('n1', 'approve')).kind).toBe('failed');
	});

	it('tells a refused dismiss of a blocking approval apart from a failure', async () => {
		const store = new NotificationStore(
			fakeApi({ dismiss: vi.fn(async () => Promise.reject(new ApiError(409, 'answer it'))) })
		);
		store.upsert(notice('n1'));
		expect(await store.dismiss('n1')).toBe('blocking');
		expect(ids(store)).toEqual(['n1']);
		const flaky = new NotificationStore(
			fakeApi({ dismiss: vi.fn(async () => Promise.reject(new ApiError(502, 'down'))) })
		);
		expect(await flaky.dismiss('n1')).toBe('failed');
	});

	it('a dismissed notice leaves the list', async () => {
		const store = new NotificationStore(
			fakeApi({
				dismiss: vi.fn(async () => ({
					notification: notice('n1', { state: 'resolved', decision: 'dismissed' }),
					applied: true
				}))
			})
		);
		store.upsert(notice('n1'));
		expect(await store.dismiss('n1')).toBe('dismissed');
		expect(store.items).toEqual([]);
	});

	it('marks read once, and only notices that are unread', async () => {
		const api = fakeApi();
		const store = new NotificationStore(api);
		store.upsert(notice('n1'));
		store.upsert(notice('n2', { state: 'read' }));
		await Promise.all([store.markRead('n1'), store.markRead('n1'), store.markRead('n2')]);
		expect(api.read).toHaveBeenCalledTimes(1);
		expect(api.read).toHaveBeenCalledWith('n1');
		expect(store.items.find((n) => n.id === 'n1')?.state).toBe('read');
	});

	it('read-all takes the server count and reloads', async () => {
		const api = fakeApi({ readAll: vi.fn(async () => ({ read: 2, unread: 1 })) });
		const store = new NotificationStore(api);
		store.setUnread(3);
		await store.readAll();
		expect(api.list).toHaveBeenCalledOnce();
		expect(store.unread).toBe(0);
	});

	it('read-all leaves answerable notices read but still asking', async () => {
		const read = [
			notice('n1', { state: 'read' }),
			notice('n2', { state: 'read', kind: 'outcome_result', actions: [], body: null })
		];
		const api = fakeApi({
			readAll: vi.fn(async () => ({ read: 2, unread: 0 })),
			list: vi.fn(async () => ({ notifications: read, unread: 0, has_more: false }))
		});
		const store = new NotificationStore(api);
		store.setUnread(2);
		await store.readAll();
		expect(api.readAll).toHaveBeenCalledOnce();
		expect(store.unread).toBe(0);
		const item = (id: string) => toItem(store.items.find((n) => n.id === id)!);
		const [ask, info] = [item('n1'), item('n2')];
		expect(ask).toMatchObject({ unread: false, text: 'Needs an answer · ship it', icon: 'help' });
		expect(ask.state).toBeUndefined();
		expect(ask.actions?.map((a) => a.id)).toEqual(['approve', 'reject']);
		expect(info).toMatchObject({ unread: false, text: undefined, icon: undefined });
		expect(info.actions).toEqual([]);
	});

	it('a notice needs an answer only while it is open and offers actions', () => {
		expect(needsAnswer(notice('n1'))).toBe(true);
		expect(needsAnswer(notice('n1', { state: 'read', body: null }))).toBe(true);
		expect(toItem(notice('n1', { state: 'read', body: null })).text).toBe('Needs an answer');
		expect(needsAnswer(notice('n1', { actions: [] }))).toBe(false);
		expect(needsAnswer(notice('n1', { state: 'acted', decision: 'approve' }))).toBe(false);
		expect(needsAnswer(notice('n1', { state: 'expired' }))).toBe(false);
		expect(
			needsAnswer(notice('n1', { kind: 'dispatch_failed', subject_kind: 'dispatch', subject_id: 'r', actions: [] }))
		).toBe(false);
	});

	it('maps a record onto the kit item', () => {
		const item = toItem(notice('n1', { severity: 'critical' }));
		expect(item).toMatchObject({
			id: 'n1',
			title: 'notice n1',
			text: 'Needs an answer · ship it',
			icon: 'help',
			unread: true
		});
		expect(item.tone).toBe('danger');
		expect(item.state).toBeUndefined();
		expect(item.actions?.map((a) => [a.id, a.variant, a.requiresConfirm])).toEqual([
			['approve', undefined, undefined],
			['reject', 'danger', true]
		]);
		expect(toItem(notice('n2', { state: 'read', actions: 'junk' })).actions).toEqual([]);
	});
});

describe('notice targets', () => {
	const failure = (over: Partial<Notice> = {}) =>
		notice('n1', {
			kind: 'dispatch_failed',
			session_id: null,
			subject_kind: 'dispatch',
			subject_id: 'd-1',
			actions: [],
			...over
		});

	it('opens the session at the message that raised the notice', () => {
		expect(noticeHref(notice('n1', { anchor_seq: 42 }))).toBe('/sessions?session=s-1&seq=42');
		expect(toItem(notice('n1', { anchor_seq: 42 })).href).toBe('/sessions?session=s-1&seq=42');
	});

	it('opens the session alone when no message is anchored', () => {
		expect(noticeHref(notice('n1'))).toBe('/sessions?session=s-1');
		expect(noticeHref(notice('n1', { anchor_seq: null }))).toBe('/sessions?session=s-1');
	});

	it('opens the failure record of a dispatch that never registered', () => {
		expect(noticeHref(failure())).toBe('/dispatch-failures?record=d-1');
		expect(noticeHref(failure({ session_id: 'd-1' })), 'a registered one opens itself').toBe(
			'/sessions?session=d-1'
		);
	});

	it('offers no target, so no open affordance, when the notice has none', () => {
		const bare = notice('n1', { session_id: null, kind: 'system_alert' });
		expect(noticeHref(bare)).toBeUndefined();
		expect(toItem(bare).href).toBeUndefined();
		expect(noticeHref(failure({ subject_kind: 'user_action' }))).toBeUndefined();
	});

	it('a failed dispatch offers re-dispatch and details until it is closed', () => {
		expect(toItem(failure()).actions?.map((a) => a.id)).toEqual(['redispatch', 'details']);
		expect(toItem(notice('n2', { kind: 'system_alert', actions: [] })).actions).toEqual([]);
		expect(toItem(failure({ state: 'resolved', decision: 'dismissed' })).actions).toEqual([]);
	});
});
