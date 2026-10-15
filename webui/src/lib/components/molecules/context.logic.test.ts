import { describe, expect, it } from 'vitest';
import {
	chipsOf,
	contextAuto,
	dropLast,
	fuzzyRank,
	notePicks,
	reconcilePrecheck,
	togglePick,
	type Pickable
} from './context.logic';

const item = (name: string, title: string, tags: string[] = [], kind = 'memory'): Pickable => ({
	id: `id-${name}`,
	kind,
	name,
	title,
	tags
});

const worktrees = item('worktrees', 'Use worktrees', ['git']);
const backend = item('rust-backend', 'Rust backend conventions');
const rubber = item('rubber', 'Rubber duck');
const deploy = item('deploy', 'Zeta');
const alpha = item('alpha', 'Deploy notes', ['ops']);
const all = [worktrees, backend, rubber, deploy, alpha];
const names = (xs: Pickable[]) => xs.map((x) => x.name);

describe('fuzzyRank', () => {
	it('finds a slug by its initials', () => {
		expect(names(fuzzyRank('wt', all))).toEqual(['worktrees']);
	});

	it('ranks word-start hits above mid-word ones', () => {
		expect(names(fuzzyRank('rb', all))).toEqual(['rust-backend', 'rubber']);
	});

	it('ranks a name match above the same title match', () => {
		expect(names(fuzzyRank('deploy', all))).toEqual(['deploy', 'alpha']);
	});

	it('matches tags', () => {
		expect(names(fuzzyRank('ops', all))).toEqual(['alpha']);
	});

	it('is case-insensitive and ignores spaces', () => {
		expect(names(fuzzyRank('WT', all))).toEqual(['worktrees']);
		expect(names(fuzzyRank(' r b ', all))).toEqual(['rust-backend', 'rubber']);
	});

	it('keeps every item in title order for an empty query', () => {
		expect(names(fuzzyRank('', all))).toEqual(['alpha', 'rubber', 'rust-backend', 'worktrees', 'deploy']);
		expect(names(fuzzyRank('   ', all))).toHaveLength(all.length);
	});

	it('returns nothing when nothing matches', () => {
		expect(fuzzyRank('zzzq', all)).toEqual([]);
	});
});

describe('togglePick', () => {
	const p1 = item('p1', 'Prompt one', [], 'prompt');
	const p2 = item('p2', 'Prompt two', [], 'prompt');
	const items = [...all, p1, p2];

	it('adds and removes memories', () => {
		const once = togglePick([], worktrees, items);
		expect(once).toEqual(['worktrees']);
		expect(togglePick([...once, 'alpha'], worktrees, items)).toEqual(['alpha']);
	});

	it('keeps a single prompt slot', () => {
		const v = togglePick(['worktrees', 'p1'], p2, items);
		expect(v).toEqual(['worktrees', 'p2']);
		expect(togglePick(v, p2, items)).toEqual(['worktrees']);
	});

	it('keys by id when asked', () => {
		expect(togglePick([], p1, items, 'id')).toEqual(['id-p1']);
		expect(togglePick(['id-p1'], p2, items, 'id')).toEqual(['id-p2']);
	});
});

describe('chips', () => {
	it('drops the last unlocked chip', () => {
		expect(dropLast(['a', 'b', 'c'])).toEqual(['a', 'b']);
		expect(dropLast(['a', 'b'], ['b'])).toEqual(['b']);
		expect(dropLast(['a'], ['a'])).toEqual(['a']);
		expect(dropLast([])).toEqual([]);
	});

	it('lists picked items in pick order, skipping unknown keys', () => {
		expect(names(chipsOf(['rubber', 'gone', 'worktrees'], all))).toEqual(['rubber', 'worktrees']);
		expect(names(chipsOf(['id-deploy'], all, 'id'))).toEqual(['deploy']);
	});
});

describe('scope pre-check', () => {
	it('checks matches, honours an uncheck, and follows the coordinates', () => {
		let s = reconcilePrecheck({ picks: ['x'], auto: [], dismissed: [] }, ['a', 'b']);
		expect(s.picks).toEqual(['x', 'a', 'b']);
		expect(contextAuto(s.picks, ['a', 'b'])).toBe(true);

		s = notePicks(s, ['x', 'b']);
		expect(s.auto).toEqual(['b']);
		expect(s.dismissed).toEqual(['a']);
		expect(contextAuto(s.picks, ['a', 'b'])).toBe(false);

		expect(reconcilePrecheck(s, ['a', 'b']).picks).toEqual(['x', 'b']);

		s = reconcilePrecheck(s, ['b', 'c']);
		expect(s.picks).toEqual(['x', 'b', 'c']);
		expect(s.dismissed).toEqual([]);

		s = reconcilePrecheck(s, []);
		expect(s.picks).toEqual(['x']);
		expect(s.auto).toEqual([]);
	});

	it('keeps a match the user had picked themselves', () => {
		const s = reconcilePrecheck({ picks: ['a'], auto: [], dismissed: [] }, ['a']);
		expect(s.auto).toEqual([]);
		expect(reconcilePrecheck(s, []).picks).toEqual(['a']);
	});

	it('leaves auto on with no matches', () => {
		expect(contextAuto([], [])).toBe(true);
	});
});
