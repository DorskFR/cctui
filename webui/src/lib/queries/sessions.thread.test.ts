import { describe, expect, it } from 'vitest';
import { qk } from './keys';
import { threadForks } from './sessions';

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
