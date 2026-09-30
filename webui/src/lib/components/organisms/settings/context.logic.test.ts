import { describe, expect, it } from 'vitest';
import {
	contextProblems,
	newContextItem,
	scopeSummary,
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

	it('requires a target for every scope but user, and an absolute path', () => {
		expect(contextProblems({ ...draft(), scope: 'machine' })).toContain('scope_ref');
		expect(contextProblems({ ...draft(), scope: 'path', scope_ref: 'rel' })).toContain(
			'abs_path'
		);
		expect(
			contextProblems({ ...draft(), scope: 'path', scope_ref: '/w/repo' })
		).toEqual([]);
		expect(contextProblems({ ...draft(), scope: 'user', scope_ref: null })).toEqual([]);
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
			tags: ['rust'],
			enabled: true
		});
	});
});

describe('scopeSummary', () => {
	it('reads as when the item applies', () => {
		expect(scopeSummary(item())).toBe('always');
		expect(scopeSummary(item({ scope: 'path', scope_ref: '/w/repo' }))).toBe('path: /w/repo');
	});
});
