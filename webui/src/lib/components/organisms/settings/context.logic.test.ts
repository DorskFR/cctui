import { describe, expect, it } from 'vitest';
import {
	bulkRequest,
	bulkResult,
	contextProblems,
	emptyMatch,
	foreignMachines,
	groupMachines,
	itemFilter,
	lineDiff,
	matchSummary,
	newContextItem,
	parsePaths,
	pendingChanges,
	RULE_BODY_MAX,
	slugify,
	toggleAll,
	toSpec
} from './context.logic';
import type { ContextItem } from '@bindings/ContextItem';

const item = (over: Partial<ContextItem> = {}): ContextItem => ({
	id: 'i-1',
	user_id: 'u-1',
	kind: 'memory',
	name: 'house-style',
	title: 'House style',
	body: 'be terse',
	scope: 'user',
	scope_ref: null,
	match: emptyMatch(),
	pending: null,
	created_by_session: null,
	updated_by_session: null,
	tags: [],
	enabled: true,
	version: 2,
	created_at: '2026-09-30T00:00:00Z',
	updated_at: '2026-09-30T00:00:00Z',
	...over
});

describe('slugify', () => {
	it('makes a title into a name the server accepts', () => {
		expect(slugify('House style')).toBe('house-style');
		expect(slugify('  Réglages, v2!  ')).toBe('reglages-v2');
		expect(slugify('***')).toBe('');
		expect(slugify('x'.repeat(80))).toHaveLength(64);
	});
});

describe('contextProblems', () => {
	const draft = () => ({ ...newContextItem('memory'), name: 'ok', title: 'Ok', body: 'text' });

	it('accepts a complete user-scoped draft', () => {
		expect(contextProblems(draft())).toEqual([]);
	});

	it('requires a title, a slug name and a body', () => {
		expect(contextProblems({ ...draft(), title: '  ' })).toContain('title');
		expect(contextProblems({ ...draft(), body: '  ' })).toContain('body');
		for (const bad of ['', '-lead', 'has space', 'Upper', 'under_score']) {
			expect(contextProblems({ ...draft(), name: bad })).toContain('name');
		}
	});

	it('requires absolute match paths', () => {
		const match = { ...emptyMatch(), paths: ['/w/repo', 'rel'] };
		expect(contextProblems({ ...draft(), match })).toContain('abs_path');
		expect(contextProblems({ ...draft(), match: { ...match, paths: ['/w/repo'] } })).toEqual([]);
	});

	it('caps a rule body at 4 KiB of UTF-8, and a memory far higher', () => {
		const rule = { ...draft(), kind: 'rule' };
		expect(contextProblems({ ...rule, body: 'x'.repeat(RULE_BODY_MAX) })).toEqual([]);
		expect(contextProblems({ ...rule, body: 'x'.repeat(RULE_BODY_MAX + 1) })).toContain('body_size');
		expect(contextProblems({ ...rule, body: 'é'.repeat(RULE_BODY_MAX / 2 + 1) })).toContain(
			'body_size'
		);
		expect(contextProblems({ ...draft(), body: 'x'.repeat(RULE_BODY_MAX + 1) })).toEqual([]);
	});
});

describe('toSpec', () => {
	it('drops the server-owned fields and keeps the editable ones', () => {
		const spec = toSpec(item({ tags: ['rust'] }));
		expect(spec).toEqual({
			kind: 'memory',
			name: 'house-style',
			title: 'House style',
			body: 'be terse',
			scope: 'user',
			scope_ref: null,
			match: emptyMatch(),
			tags: ['rust'],
			enabled: true
		});
	});
});

const words = { mine: 'my machines', allUsers: "all users' machines (admin)", dispatches: '+ my dispatches' };

describe('matchSummary', () => {
	it('names the reach first, never a bare everywhere', () => {
		expect(matchSummary(emptyMatch(), words)).toBe('my machines');
		expect(matchSummary(null, words)).toBe('my machines');
		expect(
			matchSummary(
				{ machines: ['m1'], paths: ['/a', '/b'], labels: [], adapters: ['codex'] },
				words,
				{ machine: (id) => `box-${id}` }
			)
		).toBe('my machines · box-m1 · /a | /b · codex');
		expect(matchSummary({ ...emptyMatch(), reach: 'all_users', dispatch: true }, words)).toBe(
			"all users' machines (admin) + my dispatches"
		);
		expect(matchSummary({ ...emptyMatch(), dispatch: true, dispatchers: ['kube'] }, words)).toBe(
			'my machines + my dispatches · kube'
		);
	});
});

describe('groupMachines', () => {
	const machines = [
		{ id: 'b1', owner: 'bob', name: 'zeta' },
		{ id: 'm2', owner: 'me', name: 'beta' },
		{ id: 'a1', owner: 'amy', name: 'one' },
		{ id: 'm1', owner: 'me', name: 'alpha' }
	];
	it('puts my machines first, then each other owner by name', () => {
		const groups = groupMachines(machines, 'me', (id) => id.toUpperCase());
		expect(groups.map((g) => [g.owner, g.mine, g.machines.map((x) => x.id)])).toEqual([
			['me', true, ['m1', 'm2']],
			['amy', false, ['a1']],
			['bob', false, ['b1']]
		]);
	});
	it('flags listed machines of other users', () => {
		expect(foreignMachines({ ...emptyMatch(), machines: ['m1', 'b1', 'gone'] }, machines, 'me')).toEqual(['b1']);
	});
});

describe('bulk editing', () => {
	it('builds one request per action, touching only what it names', () => {
		expect(bulkRequest(['a', 'b', 'a'], { op: 'add', key: 'machines', values: [' m1 ', ''] })).toEqual({
			ids: ['a', 'b'],
			add: { machines: ['m1'] }
		});
		expect(bulkRequest(['a'], { op: 'remove', key: 'labels', values: ['l'] })).toEqual({
			ids: ['a'],
			remove: { labels: ['l'] }
		});
		expect(bulkRequest(['a'], { op: 'reach', reach: 'all_users' })).toEqual({ ids: ['a'], set: { reach: 'all_users' } });
		expect(bulkRequest(['a'], { op: 'dispatch', on: false })).toEqual({ ids: ['a'], set: { dispatch: false } });
		expect(bulkRequest(['a'], { op: 'clear_machines' })).toEqual({ ids: ['a'], set: { clear_machines: true } });
	});
	it('scales to many items in one request', () => {
		const ids = Array.from({ length: 35 }, (_, i) => `id-${i}`);
		expect(bulkRequest(ids, { op: 'dispatch', on: true }).ids).toHaveLength(35);
	});
	it('reports per-item outcomes without hiding the failures', () => {
		const ok = item({});
		expect(
			bulkResult([
				{ id: ok.id, item: ok },
				{ id: 'x', status: 422, error: 'machine belongs to another user' },
				{ id: 'y', status: 404 }
			])
		).toEqual({
			updated: 1,
			failed: [
				{ id: 'x', error: 'machine belongs to another user' },
				{ id: 'y', error: '404' }
			]
		});
	});
	it('selects every shown row, or clears them when all are selected', () => {
		const some = toggleAll(new Set(['a', 'z']), ['a', 'b']);
		expect([...some].sort()).toEqual(['a', 'b', 'z']);
		expect([...toggleAll(some, ['a', 'b'])]).toEqual(['z']);
	});
	it('filters by title or name', () => {
		const keep = itemFilter('Deploy');
		expect(keep({ title: 'Deploy runbook', name: 'x' })).toBe(true);
		expect(keep({ title: 'Other', name: 'deploy-notes' })).toBe(true);
		expect(keep({ title: 'Other', name: 'x' })).toBe(false);
		expect(itemFilter('  ')({ title: 'a', name: 'b' })).toBe(true);
	});
});

describe('parsePaths', () => {
	it('takes one trimmed path per line', () => {
		expect(parsePaths(' /a \n\n/b\n')).toEqual(['/a', '/b']);
	});
});

describe('lineDiff', () => {
	it('keeps common lines and marks the rest', () => {
		expect(lineDiff('a\nb\nc', 'a\nx\nc\nd')).toEqual([
			{ op: 'same', text: 'a' },
			{ op: 'del', text: 'b' },
			{ op: 'add', text: 'x' },
			{ op: 'same', text: 'c' },
			{ op: 'add', text: 'd' }
		]);
		expect(lineDiff('same', 'same')).toEqual([{ op: 'same', text: 'same' }]);
	});
});

describe('pendingChanges', () => {
	it('lists only the fields the proposal changes', () => {
		const live = item({ kind: 'rule', enabled: false });
		const pending = { ...toSpec(live), enabled: true, match: { ...emptyMatch(), adapters: ['codex'] } };
		expect(pendingChanges(live, pending, (m) => matchSummary(m, words))).toEqual([
			{ field: 'enabled', before: 'false', after: 'true' },
			{ field: 'match', before: 'my machines', after: 'my machines · codex' }
		]);
	});
});
