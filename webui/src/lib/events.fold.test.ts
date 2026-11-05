import { describe, expect, it } from 'vitest';
import type { EventRecord } from '@bindings/EventRecord';
import { foldChildCreations } from './events';

let seq = 0;
function ev(kind: string, detail: EventRecord['detail'] = {}): EventRecord {
	seq += 1;
	return {
		id: seq,
		occurred_at: '2026-11-05T09:00:00Z',
		kind,
		severity: 'info',
		session_id: `s${seq}`,
		machine_id: null,
		user_id: null,
		actor: 'daemon',
		summary: `${kind} ${seq}`,
		detail
	};
}
const child = (parent: string) => ev('session.created', { origin: 'child', parent_session_id: parent });

describe('foldChildCreations', () => {
	it('folds a run of child creations and says they are all subagents', () => {
		const rows = [child('p1'), child('p1'), child('p1'), ev('session.ended')];
		const items = foldChildCreations(rows);
		expect(items.map((i) => i.type)).toEqual(['children', 'event']);
		const folded = items[0];
		expect(folded.type === 'children' && folded.events.length).toBe(3);
		expect(folded.type === 'children' && folded.allChildren).toBe(true);
	});

	it('folds mixed creations the way real feeds interleave them', () => {
		const rows = [
			ev('session.created', { origin: 'spawned' }),
			child('p1'),
			ev('session.created', { origin: 'spawned' }),
			ev('session.ended')
		];
		const items = foldChildCreations(rows);
		expect(items.map((i) => i.type)).toEqual(['children', 'event']);
		expect(items[0].type === 'children' && items[0].allChildren).toBe(false);
	});

	it('leaves a lone creation as a plain row', () => {
		const items = foldChildCreations([child('p1'), ev('session.ended')]);
		expect(items.map((i) => i.type)).toEqual(['event', 'event']);
	});

	it('keeps every event, in order', () => {
		const rows = [ev('session.ended'), child('p'), child('p'), ev('machine.online'), child('q')];
		const flat = foldChildCreations(rows).flatMap((i) => (i.type === 'children' ? i.events : [i.event]));
		expect(flat.map((r) => r.id)).toEqual(rows.map((r) => r.id));
	});
});
