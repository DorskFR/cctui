import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import { titleOf } from './view';

const session = (over: Partial<SessionListItem>): SessionListItem =>
	({ id: 'a1e5bd0f2c3d4e5f6', labels: [], working_dir: '/home/me/cctui', ...over }) as SessionListItem;

describe('titleOf', () => {
	it('names a Task subagent from its sidecar description', () => {
		// The daemon persists the sidecar `description` as the session name, so
		// the row reads as the task instead of a 6-char hash (CCT-941).
		expect(titleOf(session({ name: 'Global competitors research' }), true)).toBe(
			'Global competitors research'
		);
	});

	it('still falls back to the 6-char id for a nameless child', () => {
		expect(titleOf(session({ name: null }), true)).toBe('a1e5bd');
		expect(titleOf(session({ name: '' }), true)).toBe('a1e5bd');
	});

	it('names a top-level session from its working dir', () => {
		expect(titleOf(session({ name: null }), false)).toBe('cctui');
		expect(titleOf(session({ name: null, working_dir: '' }), false)).toBe('a1e5bd0f2c3d4e5f6');
	});
});
