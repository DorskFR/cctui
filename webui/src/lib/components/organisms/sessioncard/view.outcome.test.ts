import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { SessionOutcome } from '@bindings/SessionOutcome';
import { buildView } from './view';

const OPTS = {
	child: false,
	showMachine: false,
	now: Date.parse('2026-11-16T10:05:00Z'),
	preview: null,
	highlight: [],
	subagentCost: null,
	pendingCount: 0,
	unreadCount: 0,
	draft: false,
	draftLaunching: false
};

const RESULT: SessionOutcome = {
	id: 'o-1',
	session_id: 's-1',
	status: 'result',
	headline: 'Opened PR 12',
	source: 'tool',
	created_at: '2026-11-16T10:00:00Z'
};

function row(over: Partial<SessionListItem>): SessionListItem {
	return {
		id: 's-1',
		status: 'active',
		bucket: 'done',
		working_dir: '/w',
		last_message_text: 'last words',
		...over
	} as SessionListItem;
}

describe('the session row headline', () => {
	it('shows the declared outcome headline in place of the last message', () => {
		expect(buildView(row({ latest_outcome: RESULT }), OPTS).lastMsg).toBe('✓ Opened PR 12');
	});

	it('falls back to the last message while working or without a declared outcome', () => {
		const working = row({ bucket: 'working', latest_outcome: RESULT });
		expect(buildView(working, OPTS).lastMsg).toBe('last words');
		const inferred = { ...RESULT, status: 'ended_without_outcome', source: 'inferred' } as const;
		expect(buildView(row({ latest_outcome: inferred }), OPTS).lastMsg).toBe('last words');
		expect(buildView(row({}), OPTS).lastMsg).toBe('last words');
	});
});

describe('the session row unread count', () => {
	it('drops the unread count once the session is done', () => {
		expect(buildView(row({ bucket: 'done' }), { ...OPTS, unreadCount: 3 }).unreadCount).toBe(0);
		expect(buildView(row({ bucket: 'blocked' }), { ...OPTS, unreadCount: 3 }).unreadCount).toBe(3);
	});
});

describe('the session row status tag', () => {
	it('tags a new session but not an archived one', () => {
		expect(buildView(row({ status: 'new' }), OPTS).showStatusBadge).toBe(true);
		expect(buildView(row({ status: 'archived' }), OPTS).showStatusBadge).toBe(false);
	});
});
