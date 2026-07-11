import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import {
	buildSessionSearchSchema,
	matchesClientFilters,
	SERVER_FIELDS,
	splitQuery
} from './searchSchema';

const schema = buildSessionSearchSchema(async () => []);

function session(over: Partial<SessionListItem>): SessionListItem {
	return {
		id: 'sess-abc',
		registered_at: '2026-07-01T12:00:00Z',
		...over
	} as SessionListItem;
}

describe('splitQuery', () => {
	it('keeps server-evaluable clauses and free text in serverQuery', () => {
		const { serverQuery, clientFilters } = splitQuery('status:active foo machine:box', schema);
		expect(serverQuery).toBe('status:active foo machine:box');
		expect(clientFilters).toHaveLength(0);
	});

	it('strips client-only clauses (id, created) from serverQuery', () => {
		const { serverQuery, clientFilters } = splitQuery('id:abc status:active', schema);
		expect(serverQuery).toBe('status:active');
		expect(clientFilters.map((f) => f.field)).toEqual(['id']);
	});

	it('resolves aliases to canonical server fields', () => {
		const { serverQuery, clientFilters } = splitQuery('label:urgent name:api', schema);
		expect(clientFilters).toHaveLength(0);
		expect(serverQuery).toBe('label:urgent name:api');
	});

	it('leaves a pure-client query with an empty serverQuery', () => {
		const { serverQuery, clientFilters } = splitQuery('id:zzz', schema);
		expect(serverQuery).toBe('');
		expect(clientFilters).toHaveLength(1);
	});
});

describe('matchesClientFilters', () => {
	it('matches an id substring (contains)', () => {
		const { clientFilters } = splitQuery('id:abc', schema);
		expect(matchesClientFilters(session({ id: 'sess-abc-1' }), clientFilters)).toBe(true);
		expect(matchesClientFilters(session({ id: 'other' }), clientFilters)).toBe(false);
	});

	it('honours created date bounds', () => {
		const early = session({ registered_at: '2026-06-01T00:00:00Z' });
		const late = session({ registered_at: '2026-08-01T00:00:00Z' });
		const { clientFilters } = splitQuery('created>=2026-07-01', schema);
		expect(matchesClientFilters(early, clientFilters)).toBe(false);
		expect(matchesClientFilters(late, clientFilters)).toBe(true);
	});
});

describe('SERVER_FIELDS', () => {
	it('mirrors the cctui-query registry field set', () => {
		expect([...SERVER_FIELDS].sort()).toEqual(
			['account', 'adapter', 'dir', 'effort', 'machine', 'model', 'pinned', 'status', 'tag', 'title'].sort()
		);
	});
});
