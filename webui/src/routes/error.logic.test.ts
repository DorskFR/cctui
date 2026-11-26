import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import { errorCopy, recentSessions } from './error.logic.ts';

const s = (id: string, at: string, extra: Partial<SessionListItem> = {}) =>
	({ id, working_dir: `/w/${id}`, status: 'active', last_activity_at: at, ...extra }) as SessionListItem;

describe('errorCopy', () => {
	it('names the missing path on a 404', () => {
		expect(errorCopy(404, false, '/nope').body).toContain('/nope');
	});
	it('says a missing session was deleted, not that the page is unknown', () => {
		expect(errorCopy(404, true, '/sessions/x').title).not.toBe(errorCopy(404, false, '/x').title);
	});
});

describe('recentSessions', () => {
	it('lists the most recently active live sessions first', () => {
		const list = [s('a', '2026-11-01'), s('b', '2026-11-03'), s('c', '2026-11-02', { status: 'archived' })];
		expect(recentSessions(list, 5).map((x) => x.id)).toEqual(['b', 'a']);
	});
});
