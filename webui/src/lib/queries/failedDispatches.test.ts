import { describe, expect, it } from 'vitest';
import type { FailedDispatch } from '@bindings/FailedDispatch';
import { groupFailures, RECENT_FAILURE_HOURS } from './failedDispatches';

const now = Date.parse('2026-10-21T12:00:00Z');
const hoursAgo = (h: number) => new Date(now - h * 3_600_000).toISOString();

function failure(id: string, ageHours: number, over: Partial<FailedDispatch> = {}): FailedDispatch {
	return {
		session_id: id,
		dispatcher: 'kube',
		reason: 'crashed',
		dispatched_at: hoursAgo(ageHours),
		...over
	} as FailedDispatch;
}

const ids = (list: FailedDispatch[]) => list.map((d) => d.session_id);

describe('groupFailures', () => {
	it('collapses unresolved failures past the recent window into the older group', () => {
		const g = groupFailures(
			[failure('fresh', 1), failure('edge', RECENT_FAILURE_HOURS), failure('stale', RECENT_FAILURE_HOURS + 1)],
			now
		);
		expect(ids(g.active)).toEqual(['fresh', 'edge']);
		expect(ids(g.older)).toEqual(['stale']);
		expect(g.resolved).toEqual([]);
	});

	it('shows a recent superseded failure as resolved and drops an old one', () => {
		const g = groupFailures(
			[failure('rerun', 2, { superseded_by: 'next' }), failure('old-rerun', 48, { superseded_by: 'next2' })],
			now
		);
		expect(ids(g.resolved)).toEqual(['rerun']);
		expect(g.active).toEqual([]);
		expect(g.older).toEqual([]);
	});

	it('leaves dismissed failures out of every group', () => {
		const g = groupFailures([failure('gone', 1, { dismissed: true }), failure('gone-old', 30, { dismissed: true })], now);
		expect([...g.active, ...g.resolved, ...g.older]).toEqual([]);
	});

	it('takes the window as a parameter', () => {
		const g = groupFailures([failure('a', 3)], now, 2);
		expect(ids(g.older)).toEqual(['a']);
	});
});
