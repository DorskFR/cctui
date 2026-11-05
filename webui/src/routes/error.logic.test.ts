import { describe, expect, it } from 'vitest';
import { isNotFound, recentSessions } from './error.logic';

const s = (id: string, at: string | null, extra: Record<string, unknown> = {}) =>
	({ id, status: 'active', parent_id: null, last_activity_at: at, registered_at: null, ...extra }) as const;

describe('recentSessions', () => {
	it('lists the most recently active sessions first, capped', () => {
		const list = [s('a', '2026-11-01T00:00:00Z'), s('b', '2026-11-03T00:00:00Z'), s('c', '2026-11-02T00:00:00Z')];
		expect(recentSessions(list, 2).map((x) => x.id)).toEqual(['b', 'c']);
	});

	it('skips archived sessions and subagents', () => {
		const list = [
			s('a', '2026-11-01T00:00:00Z', { status: 'archived' }),
			s('b', '2026-11-02T00:00:00Z', { parent_id: 'x' }),
			s('c', null, { registered_at: '2026-10-01T00:00:00Z' })
		];
		expect(recentSessions(list).map((x) => x.id)).toEqual(['c']);
	});

	it('is empty before the list loads', () => {
		expect(recentSessions(undefined)).toEqual([]);
	});
});

describe('isNotFound', () => {
	it('is true for 404 only', () => {
		expect(isNotFound(404)).toBe(true);
		expect(isNotFound(500)).toBe(false);
	});
});
