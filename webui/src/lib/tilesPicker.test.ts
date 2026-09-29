import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import { buildSessionSearchSchema } from './searchSchema';
import { pickerMatches } from './tilesPicker';

const schema = buildSessionSearchSchema(async () => []);

function row(over: Partial<SessionListItem> = {}): SessionListItem {
	return {
		id: 'sess-1',
		name: 'Add pagination',
		working_dir: '/home/me/api',
		machine_id: 'box',
		status: 'active',
		adapter_id: 'claude-code',
		pinned: false,
		labels: [],
		...over
	} as unknown as SessionListItem;
}

const ok = (raw: string, s = row()) => pickerMatches(s, raw, schema);

describe('pickerMatches', () => {
	it('matches everything on an empty query', () => {
		expect(ok('')).toBe(true);
		expect(ok('   ')).toBe(true);
	});

	it('ANDs free-text terms over name, dir and id', () => {
		expect(ok('pagination')).toBe(true);
		expect(ok('api')).toBe(true);
		expect(ok('sess-1')).toBe(true);
		expect(ok('pagination api')).toBe(true);
		expect(ok('pagination nope')).toBe(false);
	});

	it('is case-insensitive', () => {
		expect(ok('PAGINATION')).toBe(true);
	});

	it('honours the fields a list row actually carries', () => {
		expect(ok('status:active')).toBe(true);
		expect(ok('status:archived')).toBe(false);
		expect(ok('adapter:codex')).toBe(false);
		expect(ok('machine:box')).toBe(true);
		expect(ok('dir:/home/me')).toBe(true);
		expect(ok('title:pagination')).toBe(true);
		expect(ok('title:nothing')).toBe(false);
	});

	it('reads pinned as a boolean', () => {
		expect(ok('pinned:true')).toBe(false);
		expect(ok('pinned:false')).toBe(true);
		expect(ok('pinned:true', row({ pinned: true }))).toBe(true);
	});

	it('negates', () => {
		expect(ok('status!=archived')).toBe(true);
		expect(ok('status!=active')).toBe(false);
		expect(ok('title!:pagination')).toBe(false);
	});

	it('never empties the list over a clause a row cannot answer', () => {
		expect(ok('tag:backend')).toBe(true);
		expect(ok('account:someone')).toBe(true);
	});
});
