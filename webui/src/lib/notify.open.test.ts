// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { notify } from './notify.svelte';

describe('notify open-session set', () => {
	it('tracks every pane holding a session, not just one', () => {
		const offA = notify.holdOpen('a');
		const offB = notify.holdOpen('b');
		expect([...notify.openSessionIds].sort()).toEqual(['a', 'b']);
		offA();
		expect([...notify.openSessionIds]).toEqual(['b']);
		offB();
		expect([...notify.openSessionIds]).toEqual([]);
	});

	it('ref-counts one session held twice, so closing one pane keeps it held', () => {
		const first = notify.holdOpen('dup');
		const second = notify.holdOpen('dup');
		expect([...notify.openSessionIds]).toEqual(['dup']);
		first();
		expect([...notify.openSessionIds], 'a tile still shows it').toEqual(['dup']);
		second();
		expect([...notify.openSessionIds]).toEqual([]);
	});

	it('a stale teardown cannot drive the count negative', () => {
		const off = notify.holdOpen('x');
		off();
		off();
		expect([...notify.openSessionIds]).toEqual([]);
		notify.holdOpen('x')();
		expect([...notify.openSessionIds]).toEqual([]);
	});
});
