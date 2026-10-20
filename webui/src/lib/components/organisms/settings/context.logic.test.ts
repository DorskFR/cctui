import { describe, expect, it } from 'vitest';
import {
	contextProblems,
	emptyMatch,
	lineDiff,
	matchSummary,
	newContextItem,
	parsePaths,
	pendingChanges,
	RULE_BODY_MAX,
	slugify,
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

describe('matchSummary', () => {
	it('reads as when the item applies', () => {
		expect(matchSummary(emptyMatch(), 'always')).toBe('always');
		expect(matchSummary(null, 'always')).toBe('always');
		expect(
			matchSummary(
				{ machines: ['m1'], paths: ['/a', '/b'], labels: [], adapters: ['codex'] },
				'always',
				{ machine: (id) => `box-${id}` }
			)
		).toBe('box-m1 · /a | /b · codex');
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
		expect(pendingChanges(live, pending, (m) => matchSummary(m, 'always'))).toEqual([
			{ field: 'enabled', before: 'false', after: 'true' },
			{ field: 'match', before: 'always', after: 'codex' }
		]);
	});
});
