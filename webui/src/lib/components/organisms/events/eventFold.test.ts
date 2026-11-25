import { describe, expect, it } from 'vitest';
import type { EventRecord } from '@bindings/EventRecord';
import { foldChildCreations, isChildCreation } from './eventFold';

const ev = (id: number, kind: string, detail: Record<string, unknown> | null = null, machine = 'm1') =>
	({ id, kind, detail, machine_id: machine }) as unknown as EventRecord;
const child = (id: number, parent = 'p1', machine = 'm1') =>
	ev(id, 'session.created', { origin: 'child', parent_session_id: parent }, machine);

describe('foldChildCreations', () => {
	it('recognises only child creations', () => {
		expect(isChildCreation(child(1))).toBe(true);
		expect(isChildCreation(ev(2, 'session.created', { origin: 'spawned' }))).toBe(false);
		expect(isChildCreation(ev(3, 'session.ended', { origin: 'child' }))).toBe(false);
	});

	it('folds adjacent creations under one parent and keeps the rest in order', () => {
		const items = foldChildCreations([ev(9, 'session.ended'), child(8), child(7), child(6), ev(5, 'machine.online')]);
		expect(items.map((i) => i.type)).toEqual(['row', 'fold', 'row']);
		const fold = items[1];
		expect(fold.type === 'fold' && fold.events.map((e) => e.id)).toEqual([8, 7, 6]);
		expect(fold.type === 'fold' && fold.machineId).toBe('m1');
	});

	it('leaves a lone child creation as a row', () => {
		expect(foldChildCreations([child(2), ev(1, 'session.ended')]).map((i) => i.type)).toEqual(['row', 'row']);
	});

	it('splits runs on a parent change and drops the machine when it differs', () => {
		const items = foldChildCreations([child(4, 'a'), child(3, 'a', 'm2'), child(2, 'b'), child(1, 'b')]);
		expect(items.map((i) => (i.type === 'fold' ? i.events.length : 1))).toEqual([2, 2]);
		expect(items[0].type === 'fold' && items[0].machineId).toBeNull();
		expect(items[1].type === 'fold' && items[1].machineId).toBe('m1');
	});
});
