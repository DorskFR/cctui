// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ws } from './ws.svelte';

const S = 'ws-refcount-session';

afterEach(() => {
	while (ws.subscriberCount(S) > 0) ws.unsubscribe(S);
});

describe('ws session subscriptions are ref-counted', () => {
	it('counts every holder, and only the last one out releases it', () => {
		expect(ws.subscriberCount(S)).toBe(0);
		ws.subscribe(S);
		expect(ws.subscriberCount(S)).toBe(1);
		ws.subscribe(S);
		expect(ws.subscriberCount(S)).toBe(2);

		ws.unsubscribe(S);
		expect(ws.subscriberCount(S), 'the drawer closing must not drop the tile').toBe(1);
		ws.unsubscribe(S);
		expect(ws.subscriberCount(S)).toBe(0);
	});

	it('ignores an unsubscribe for a session nobody holds', () => {
		ws.unsubscribe('never-subscribed');
		expect(ws.subscriberCount('never-subscribed')).toBe(0);
	});

	it('re-subscribing after the count reached zero starts a fresh count', () => {
		ws.subscribe(S);
		ws.unsubscribe(S);
		expect(ws.subscriberCount(S)).toBe(0);
		ws.subscribe(S);
		expect(ws.subscriberCount(S)).toBe(1);
	});
});

describe('ws.onRefocus', () => {
	it('installs one listener pair for any number of subscribers', () => {
		const add = vi.spyOn(window, 'addEventListener');
		const focusAdds = () => add.mock.calls.filter(([type]) => type === 'focus').length;
		try {
			const offA = ws.onRefocus(() => {});
			const offB = ws.onRefocus(() => {});
			expect(focusAdds(), 'N tiles must not mean N window listeners').toBe(1);
			offA();
			offB();
			const offC = ws.onRefocus(() => {});
			expect(focusAdds(), 'reinstalled once the last subscriber left').toBe(2);
			offC();
		} finally {
			add.mockRestore();
		}
	});

	it('fans one focus event out to every subscriber', () => {
		const seen: string[] = [];
		const offA = ws.onRefocus(() => seen.push('a'));
		const offB = ws.onRefocus(() => seen.push('b'));
		window.dispatchEvent(new Event('focus'));
		expect(seen).toEqual(['a', 'b']);
		offA();
		offB();
		window.dispatchEvent(new Event('focus'));
		expect(seen, 'a torn-down subscriber hears nothing').toEqual(['a', 'b']);
	});
});
