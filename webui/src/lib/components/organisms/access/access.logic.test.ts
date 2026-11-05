import { describe, expect, it } from 'vitest';
import {
	ALL_SCOPES,
	filterByName,
	keyIcon,
	scopeCells,
	splitRevoked,
	userMetaWords,
	userStateWords,
	visibleRows
} from './access.logic';

const row = (id: string, revoked_at: string | null) => ({ id, revoked_at });

describe('scopeCells', () => {
	it('always returns the four scopes in ladder order', () => {
		expect(scopeCells([]).map((c) => c.name)).toEqual([...ALL_SCOPES]);
	});

	it('flags the granted ones and leaves the rest missing', () => {
		const cells = scopeCells(['dispatch', 'read']);
		expect(cells.filter((c) => c.granted).map((c) => c.name)).toEqual(['read', 'dispatch']);
		expect(cells.filter((c) => !c.granted).map((c) => c.name)).toEqual(['enroll', 'admin']);
	});

	it('ignores scopes outside the ladder', () => {
		expect(scopeCells(['read', 'nonsense']).filter((c) => c.granted)).toHaveLength(1);
	});
});

describe('splitRevoked', () => {
	it('groups revoked rows after the live ones, preserving order', () => {
		const rows = [row('a', null), row('b', '2026-01-01'), row('c', null)];
		const { active, revoked } = splitRevoked(rows);
		expect(active.map((r) => r.id)).toEqual(['a', 'c']);
		expect(revoked.map((r) => r.id)).toEqual(['b']);
	});

	it('hides the revoked group until the filter is on', () => {
		const rows = [row('a', null), row('b', '2026-01-01')];
		expect(visibleRows(rows, false).map((r) => r.id)).toEqual(['a']);
		expect(visibleRows(rows, true).map((r) => r.id)).toEqual(['a', 'b']);
	});
});

describe('filterByName', () => {
	it('matches case-insensitively and keeps everything on a blank query', () => {
		const users = [{ name: 'dorsk' }, { name: 'tmp-CI' }];
		expect(filterByName(users, 'ci')).toEqual([{ name: 'tmp-CI' }]);
		expect(filterByName(users, '  ')).toHaveLength(2);
	});
});

describe('row glyphs', () => {
	it('marks machine keys with a screen and everything else with a person', () => {
		expect(keyIcon('machine')).toBe('tv');
		expect(keyIcon('user')).toBe('user');
	});
});

describe('userStateWords', () => {
	it('says the state in words, not with a colour', () => {
		expect(userStateWords(0, 3)).toBe('offline');
		expect(userStateWords(0, 0)).toBe('offline');
		expect(userStateWords(1, 1)).toBe('online');
		expect(userStateWords(1, 4)).toBe('1 machine online');
		expect(userStateWords(2, 5)).toBe('2 machines online');
	});
});

describe('userMetaWords', () => {
	it('pluralises each count', () => {
		expect(userMetaWords(1, 1, 1)).toBe('1 machine · 1 account · online');
		expect(userMetaWords(5, 3, 4)).toBe('5 machines · 3 accounts · 4 machines online');
		expect(userMetaWords(0, 0, 0)).toBe('0 machines · 0 accounts · offline');
	});
});
