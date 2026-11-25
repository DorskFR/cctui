import { describe, expect, it } from 'vitest';
import type { InboxEvent } from '#lib/queries/inbox.ts';
import { presetLane, runRow, runRows, runState } from './runs';

const event = (over: Partial<InboxEvent> = {}): InboxEvent => ({
	id: 'e1',
	lane_id: 'l1',
	lane_key: 'preset:p1:default',
	source: 'schedule',
	source_id: 'p1@2026-11-16T09:05:00Z',
	body: 'Summarise the night',
	payload: null,
	status: 'delivered',
	not_before: '2026-11-16T09:05:00Z',
	expires_at: null,
	attempts: 0,
	last_error: null,
	batch_id: null,
	delivered_session_id: 'abcdef0123456789',
	delivered_as: 'new',
	created_at: '2026-11-16T09:05:00Z',
	delivered_at: '2026-11-16T09:05:02Z',
	...over
});

describe('run rows', () => {
	it('names the lane a preset runs in', () => {
		expect(presetLane('p1')).toBe('preset:p1:default');
	});

	it('classifies every status', () => {
		expect(runState({ status: 'pending', last_error: null })).toBe('queued');
		expect(runState({ status: 'claimed', last_error: null })).toBe('running');
		expect(runState({ status: 'delivered', last_error: null })).toBe('delivered');
		expect(runState({ status: 'dead', last_error: 'machine offline' })).toBe('failed');
		expect(runState({ status: 'cancelled', last_error: null })).toBe('cancelled');
	});

	it('tells a skipped fire from a cancelled one', () => {
		const skipped = runRow(
			event({
				status: 'cancelled',
				last_error: 'skipped: the previous run is still working',
				delivered_session_id: null
			})
		);
		expect(skipped).toMatchObject({ state: 'skipped', error: null, sessionId: null });
	});

	it('marks a run-now and links the session it started', () => {
		expect(runRow(event({ payload: { preset_id: 'p1', manual: true } }))).toMatchObject({
			state: 'delivered',
			manual: true,
			sessionId: 'abcdef0123456789'
		});
		expect(runRow(event()).manual).toBe(false);
	});

	it('keeps the error of a failed run', () => {
		expect(runRow(event({ status: 'dead', last_error: 'machine offline' })).error).toBe('machine offline');
	});

	it('lists the newest runs first and caps the list', () => {
		const events = Array.from({ length: 12 }, (_, i) =>
			event({ id: `e${i}`, created_at: `2026-11-${String(i + 1).padStart(2, '0')}T09:00:00Z` })
		);
		const rows = runRows(events);
		expect(rows).toHaveLength(10);
		expect(rows[0].id).toBe('e11');
		expect(rows.at(-1)?.id).toBe('e2');
	});
});
