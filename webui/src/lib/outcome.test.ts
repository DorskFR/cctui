import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { SessionOutcome } from '@bindings/SessionOutcome';
import { groupOf } from '../routes/sessions/sessions.logic';
import { freshResults, outcomeBucket, outcomeHeadline } from './outcome';

function outcome(over: Partial<SessionOutcome> = {}): SessionOutcome {
	return {
		id: 'o-1',
		session_id: 's-1',
		status: 'result',
		headline: 'Opened PR 12',
		source: 'tool',
		created_at: '2026-11-16T10:00:00Z',
		...over
	};
}

function session(over: Partial<SessionListItem>): SessionListItem {
	return { id: 's-1', bucket: 'done', status: 'active', ...over } as SessionListItem;
}

describe('the Completed bucket keys on a result outcome', () => {
	it('lands an idle session with a result in Completed', () => {
		const s = session({ latest_outcome: outcome() });
		expect(outcomeBucket(s)).toBe('done');
		expect(groupOf(s)).toBe('done');
	});

	it('keeps an idle session that ended without an outcome out of Completed', () => {
		expect(outcomeBucket(session({}))).toBe('review');
		const inferred = outcome({ status: 'ended_without_outcome', source: 'inferred' });
		expect(groupOf(session({ latest_outcome: inferred }))).toBe('review');
		expect(groupOf(session({ latest_outcome: outcome({ status: 'failed' }) }))).toBe('review');
	});

	it('files a declared needs-input outcome under Needs input', () => {
		expect(groupOf(session({ latest_outcome: outcome({ status: 'needs_input' }) }))).toBe(
			'blocked'
		);
	});

	it('leaves a live bucket alone whatever the last outcome said', () => {
		const s = session({ bucket: 'working', latest_outcome: outcome() });
		expect(outcomeBucket(s)).toBe('working');
		expect(outcomeBucket(session({ bucket: 'blocked' }))).toBe('blocked');
	});
});

describe('outcome headline and completion alerts', () => {
	it('shows a declared headline and hides the inferred stock one', () => {
		expect(outcomeHeadline(outcome())).toBe('✓ Opened PR 12');
		expect(outcomeHeadline(outcome({ status: 'failed', headline: 'no repo' }))).toBe(
			'✗ no repo'
		);
		expect(outcomeHeadline(outcome({ status: 'ended_without_outcome', source: 'inferred' })))
			.toBeNull();
		expect(outcomeHeadline(null)).toBeNull();
	});

	it('announces each result outcome once and ignores the rest', () => {
		const seen = new Set<string>();
		const done = session({ id: 'a', latest_outcome: outcome({ id: 'o-a' }) });
		const quiet = session({ id: 'b', latest_outcome: outcome({ id: 'o-b', status: 'failed' }) });
		expect(freshResults(seen, [done, quiet]).map((s) => s.id)).toEqual(['a']);
		expect(freshResults(seen, [done, quiet])).toEqual([]);
		const again = session({ id: 'a', latest_outcome: outcome({ id: 'o-a2' }) });
		expect(freshResults(seen, [again]).map((s) => s.id)).toEqual(['a']);
	});
});
