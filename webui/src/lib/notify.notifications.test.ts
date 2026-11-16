// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Notification as Notice } from '@bindings/Notification';
import type { SessionListItem } from '@bindings/SessionListItem';

vi.mock('$app/environment', () => ({ browser: true }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

class FakeNotification {
	static permission = 'granted';
	static requestPermission = async () => 'granted';
	static shown: FakeNotification[] = [];
	closed = false;
	onclick: (() => void) | null = null;
	constructor(
		readonly title: string,
		readonly opts: { body?: string; tag?: string }
	) {
		FakeNotification.shown.push(this);
	}
	close() {
		this.closed = true;
	}
}
vi.stubGlobal('Notification', FakeNotification);

const { notify } = await import('./notify.svelte');

function notice(over: Partial<Notice> = {}): Notice {
	return {
		id: 'n1',
		user_id: 'u-1',
		kind: 'approval',
		severity: 'high',
		session_id: 's-1',
		machine_id: null,
		subject_kind: null,
		subject_id: null,
		collapse_key: 'approval:s-1',
		title: 'Release?',
		body: 'ship it',
		tags: null,
		actions: [],
		state: 'delivered',
		repeat_count: 1,
		decision: null,
		decision_note: null,
		acted_channel: null,
		acted_by: null,
		resolved_reason: null,
		created_at: '2026-11-16T10:00:00Z',
		updated_at: '2026-11-16T10:00:00Z',
		read_at: null,
		acted_at: null,
		resolved_at: null,
		expires_at: null,
		...over
	};
}

beforeEach(() => {
	FakeNotification.shown = [];
	notify.enabled = true;
	notify.permission = 'granted';
	notify.setSound(false);
});

describe('notify consumes notification frames', () => {
	it('alerts once per notice, again on a collapse repeat, and closes once read', () => {
		notify.announce(notice());
		notify.announce(notice());
		expect(FakeNotification.shown.map((n) => [n.title, n.opts.tag])).toEqual([
			['Release?', 'approval:s-1']
		]);
		notify.announce(notice({ repeat_count: 2 }));
		expect(FakeNotification.shown).toHaveLength(2);
		notify.announce(notice({ state: 'read', repeat_count: 2 }));
		expect(FakeNotification.shown[1].closed).toBe(true);
	});

	it('stays quiet for a session already on screen', () => {
		const release = notify.holdOpen('s-9');
		notify.announce(notice({ id: 'n9', session_id: 's-9' }));
		expect(FakeNotification.shown).toEqual([]);
		release();
	});

	it('the title badge is the server unread count', () => {
		notify.setUnread(3);
		expect(document.title).toBe('(3) cctui');
		notify.setUnread(0);
		expect(document.title).toBe('cctui');
	});

	it('the session-list fallback skips sessions a notification covers', () => {
		const s = (id: string) => ({ id, name: id }) as SessionListItem;
		notify.reconcile([s('a'), s('b')], new Set(['a']));
		expect(FakeNotification.shown.map((n) => n.opts.tag)).toEqual(['b']);
		notify.reconcile([], new Set());
	});
});
