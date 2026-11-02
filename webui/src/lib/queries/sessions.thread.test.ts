import { describe, expect, it } from 'vitest';
import { QueryClient } from '@tanstack/svelte-query';
import { qk } from './keys';
import { cachedThreadMembers, listedFollowups, threadForks, threadMembers } from './sessions';

const f = (id: string, parent: string) => ({ id, parent_id: parent });

describe('threadForks', () => {
	it('changes for every member when the root is followed up again', () => {
		const before = [f('a', 'r'), f('b', 'r')];
		const after = [...before, f('c', 'r')];
		for (const member of ['r', 'a', 'b']) {
			expect(threadForks(member, before)).toBe('a,b');
			expect(threadForks(member, after)).toBe('a,b,c');
		}
	});

	it('ignores follow-ups of other threads and is order independent', () => {
		const list = [f('x', 'y'), f('b', 'r'), f('a2', 'a'), f('a', 'r')];
		expect(threadForks('a2', list)).toBe('a,a2,b');
		expect(threadForks('y', list)).toBe('x');
	});

	it('reaches siblings across members missing from the list via the fetched thread', () => {
		const list = [f('a2', 'a'), f('b', 'r')];
		expect(threadForks('a2', list)).toBe('a2');
		expect(threadForks('a2', list, ['r', 'a', 'a2'])).toBe('a2,b');
	});

	it('keys every thread of one session under one prefix', () => {
		expect(qk.sessionThread('a', 'b', 'a,b').slice(0, 2)).toEqual([...qk.sessionThreadAll('a')]);
	});
});

describe('thread key inputs from cached bodies', () => {
	it('reads no members from a body that is not a thread response', () => {
		expect(threadMembers([])).toEqual([]);
		expect(threadMembers(undefined)).toEqual([]);
		expect(threadMembers(null)).toEqual([]);
		expect(threadMembers({ sessions: 'x' })).toEqual([]);
		expect(threadMembers({ sessions: [{ id: 'r' }, { id: 'a' }] })).toEqual(['r', 'a']);
	});

	it('survives a cached empty-array thread, as an unstubbed /thread answers', () => {
		const qc = new QueryClient();
		qc.setQueryData(qk.sessionThread('s', null, ''), []);
		qc.setQueryData(qk.sessionThread('s', 'n', 'n'), { sessions: [{ id: 's' }, { id: 'n' }] });
		expect(() => cachedThreadMembers(qc, 's')).not.toThrow();
		expect(new Set(cachedThreadMembers(qc, 's'))).toEqual(new Set(['s', 'n']));
		expect(cachedThreadMembers(qc, 'other')).toEqual([]);
	});

	it('takes only follow-up links from a list body and none from a malformed one', () => {
		expect(listedFollowups([])).toEqual([]);
		expect(listedFollowups(undefined)).toEqual([]);
		expect(
			listedFollowups({
				sessions: [
					{ id: 'a', parent_id: 'r', metadata: { relation: 'followup' } },
					{ id: 'k', parent_id: 'r', metadata: { relation: 'fork' } },
					{ id: 'x', parent_id: null, metadata: null }
				]
			})
		).toEqual([{ id: 'a', parent_id: 'r' }]);
	});
});
