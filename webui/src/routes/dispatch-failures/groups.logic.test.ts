import { describe, expect, it } from 'vitest';
import type { FailedDispatch } from '@bindings/FailedDispatch';
import { causeOf, groupByCause } from './groups.logic';

const d = (id: string, over: Partial<FailedDispatch> = {}): FailedDispatch =>
	({
		session_id: id,
		dispatcher: 'kube',
		reason: 'never_registered',
		dispatched_at: '2026-10-20T08:00:00Z',
		...over
	}) as FailedDispatch;

describe('groupByCause', () => {
	it('puts dispatches missing the same env keys in one group, whatever the key order', () => {
		const groups = groupByCause([
			d('a', { rerun_blocked: 'env', env_keys: ['B', 'A'] }),
			d('b', { rerun_blocked: 'env', env_keys: ['A', 'B'] }),
			d('c', { rerun_blocked: 'env', env_keys: ['C'] })
		]);
		expect(groups.map((g) => [g.key, g.rows.map((r) => r.session_id)])).toEqual([
			['env:A,B', ['a', 'b']],
			['env:C', ['c']]
		]);
		expect(groups[0].envKeys).toEqual(['A', 'B']);
	});

	it('lists re-dispatchable first and resolved last, keeping row order inside a group', () => {
		const groups = groupByCause([
			d('r1', { superseded_by: 'x', rerun_blocked: 'env', env_keys: ['A'] }),
			d('p1', { rerun_blocked: 'prompt_truncated' }),
			d('ok1'),
			d('ok2')
		]);
		expect(groups.map((g) => g.kind)).toEqual(['rerunnable', 'prompt_truncated', 'resolved']);
		expect(groups[0].rows.map((r) => r.session_id)).toEqual(['ok1', 'ok2']);
	});

	it('a superseded failure is resolved whatever blocked its re-run', () => {
		expect(causeOf(d('a', { superseded_by: 'x', rerun_blocked: 'payload_too_large' })).kind).toBe('resolved');
	});
});
