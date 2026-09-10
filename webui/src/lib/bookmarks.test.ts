import { describe, expect, it } from 'vitest';
import type { Bookmark } from '@bindings/Bookmark';
import {
	bookmarkMarkdown,
	defaultTitle,
	draftFromLine,
	isDeadLink,
	isLineBookmarked,
	lastAssistantLine,
	queryTerms,
	sourceHref
} from './bookmarks';
import type { Line } from './components/organisms/conversation/types';

function bm(over: Partial<Bookmark> = {}): Bookmark {
	return {
		id: 'b1',
		session_id: 's1',
		seq: 12,
		message_id: null,
		title: 'Wrap-up',
		body: 'the report',
		role: 'assistant',
		session_name: 'nightly',
		note: null,
		message_ts: '2026-09-09T10:00:00Z',
		created_at: '2026-09-09T10:00:01Z',
		...over
	};
}

describe('defaultTitle', () => {
	it('takes the first non-empty line', () => {
		expect(defaultTitle('\n\n  Wrap-up  \nrest')).toBe('Wrap-up');
	});

	it('is empty for a blank body', () => {
		expect(defaultTitle('\n  \n')).toBe('');
	});

	it('trims to 120 chars with an ellipsis', () => {
		const t = defaultTitle('x'.repeat(400));
		expect([...t]).toHaveLength(120);
		expect(t.endsWith('…')).toBe(true);
	});
});

describe('dead links', () => {
	it('is a dead link once the source session is gone', () => {
		expect(isDeadLink(bm())).toBe(false);
		expect(isDeadLink(bm({ session_id: null }))).toBe(true);
		expect(sourceHref(bm({ session_id: null }))).toBeNull();
	});

	it('carries seq as the focus seam when the session lives', () => {
		expect(sourceHref(bm())).toBe('/sessions/s1?seq=12');
		expect(sourceHref(bm({ seq: null }))).toBe('/sessions/s1');
	});
});

describe('bookmarkMarkdown', () => {
	it('renders title, note and body', () => {
		expect(bookmarkMarkdown(bm({ note: 'why' }))).toBe('# Wrap-up\n\n> why\n\nthe report');
	});

	it('omits an absent note', () => {
		expect(bookmarkMarkdown(bm())).toBe('# Wrap-up\n\nthe report');
	});
});

describe('queryTerms', () => {
	it('splits on whitespace and keeps quoted phrases whole', () => {
		expect(queryTerms('alpha beta')).toEqual(['alpha', 'beta']);
		expect(queryTerms('"wrap up" beta')).toEqual(['wrap up', 'beta']);
		expect(queryTerms('   ')).toEqual([]);
	});
});

describe('draftFromLine', () => {
	it('snapshots the rendered markdown, role, ts and back-link', () => {
		const ln = { role: 'assistant', ts: 1234, text: 'Done.\nresult: shipped' } as Line;
		const d = draftFromLine(ln, 's1', 'nightly');
		expect(d.body).toBe('Done.\nresult: shipped');
		expect(d.title).toBe('Done.');
		expect(d.role).toBe('assistant');
		expect(d.message_ts).toBe(1234);
		expect(d.session_id).toBe('s1');
		expect(d.session_name).toBe('nightly');
	});

	it('carries seq when the line is stamped and null when it is not', () => {
		const bare = { role: 'assistant', ts: 1, text: 'a' } as Line;
		expect(draftFromLine(bare, 's1', null).seq).toBeNull();
		const stamped = { role: 'assistant', ts: 1, text: 'a', seq: 9 } as Line & { seq: number };
		expect(draftFromLine(stamped, 's1', null).seq).toBe(9);
	});
});

describe('lastAssistantLine', () => {
	it('picks the newest non-empty assistant line', () => {
		const lines = [
			{ role: 'assistant', ts: 1, text: 'first' },
			{ role: 'assistant', ts: 2, text: 'the wrap-up' },
			{ role: 'tool', ts: 3, text: 'ls' }
		] as Line[];
		expect(lastAssistantLine(lines)?.text).toBe('the wrap-up');
	});

	it('is null when the session has no assistant prose', () => {
		expect(lastAssistantLine([{ role: 'user', ts: 1, text: 'hi' }] as Line[])).toBeNull();
	});
});

describe('isLineBookmarked', () => {
	it('matches on seq within the same session', () => {
		const ln = { role: 'assistant', ts: 1, text: 'a', seq: 12 } as Line & { seq: number };
		expect(isLineBookmarked([bm()], 's1', ln)).not.toBeNull();
		expect(isLineBookmarked([bm()], 'other', ln)).toBeNull();
	});

	it('falls back to message_id when seq is absent', () => {
		const ln = { role: 'assistant', ts: 1, text: 'a', messageId: 'msg_1' } as Line;
		expect(isLineBookmarked([bm({ seq: null, message_id: 'msg_1' })], 's1', ln)).not.toBeNull();
	});

	it('does not match an unrelated line', () => {
		const ln = { role: 'assistant', ts: 1, text: 'a' } as Line;
		expect(isLineBookmarked([bm()], 's1', ln)).toBeNull();
	});
});
