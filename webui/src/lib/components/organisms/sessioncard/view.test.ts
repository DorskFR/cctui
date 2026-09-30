import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import { needsYouOf, scheduledLaunchOf, titleOf } from './view';

const session = (over: Partial<SessionListItem>): SessionListItem =>
	({ id: 'a1e5bd0f2c3d4e5f6', labels: [], working_dir: '/home/me/cctui', ...over }) as SessionListItem;

describe('titleOf', () => {
	it('names a Task subagent from its sidecar description', () => {
		// The daemon persists the sidecar `description` as the session name, so
		// the row reads as the task instead of a 6-char hash.
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

describe('scheduledLaunchOf', () => {
	it('is null for a draft with no queued launch', () => {
		expect(scheduledLaunchOf(session({ launch_at: null }))).toBeNull();
		expect(scheduledLaunchOf(session({}))).toBeNull();
	});

	it('reads the queued launch time and its last failure', () => {
		const at = '2026-10-02T07:30:00Z';
		const got = scheduledLaunchOf(session({ launch_at: at, launch_error: 'machine offline' }));
		expect(got?.at.toISOString()).toBe(new Date(at).toISOString());
		expect(got?.error).toBe('machine offline');
		expect(got?.label).not.toBe('');
	});

	it('ignores an unparseable launch time rather than rendering Invalid Date', () => {
		expect(scheduledLaunchOf(session({ launch_at: 'never' }))).toBeNull();
	});
});

describe('needsYouOf', () => {
	it('is null when the server sent no counts, or only zeroes', () => {
		expect(needsYouOf(session({}))).toBeNull();
		expect(
			needsYouOf(session({ user_actions: { open: 0, blocking: 0, child_open: 0, child_blocking: 0 } }))
		).toBeNull();
	});

	it('reports the open items on the session itself and whether any blocks', () => {
		const got = needsYouOf(
			session({ user_actions: { open: 3, blocking: 1, child_open: 0, child_blocking: 0 } })
		);
		expect(got).toEqual({ count: 3, child: 0, blocking: true });
	});

	it('surfaces a child that is blocked on the user even when the parent is not', () => {
		const got = needsYouOf(
			session({ user_actions: { open: 0, blocking: 0, child_open: 2, child_blocking: 1 } })
		);
		expect(got).toEqual({ count: 0, child: 2, blocking: true });
	});

	it('does not treat a non-blocking child item as blocking', () => {
		const got = needsYouOf(
			session({ user_actions: { open: 1, blocking: 0, child_open: 1, child_blocking: 0 } })
		);
		expect(got?.blocking).toBe(false);
	});
});
