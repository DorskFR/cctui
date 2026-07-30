import { describe, expect, it, vi } from 'vitest';
import { decodeBase64, KeyedListeners, BoundedEventBuffer } from './ws.svelte';
import type { AgentEvent } from '@bindings/AgentEvent';

const reply = (content: string, ts: number): AgentEvent => ({
	type: 'reply',
	content,
	ts,
	seq: null
});

describe('decodeBase64 (CCT-545 PTY chunks)', () => {
	it('round-trips ASCII bytes', () => {
		expect(Array.from(decodeBase64('aGk='))).toEqual([104, 105]);
	});

	it('preserves raw non-UTF8 / control bytes (ANSI escapes)', () => {
		// ESC [ 3 1 m — a raw SGR sequence must survive byte-exact for xterm.
		const b64 = btoa(String.fromCharCode(0x1b, 0x5b, 0x33, 0x31, 0x6d, 0xff));
		expect(Array.from(decodeBase64(b64))).toEqual([0x1b, 0x5b, 0x33, 0x31, 0x6d, 0xff]);
	});

	it('decodes an empty payload to zero bytes', () => {
		expect(decodeBase64('').length).toBe(0);
	});
});

describe('KeyedListeners', () => {
	it('fans out only to listeners of the emitted key', () => {
		const reg = new KeyedListeners<number>();
		const a = vi.fn();
		const b = vi.fn();
		reg.add('s1', a);
		reg.add('s2', b);
		reg.emit('s1', 7);
		expect(a).toHaveBeenCalledWith(7);
		expect(b).not.toHaveBeenCalled();
	});

	it('supports multiple listeners per key', () => {
		const reg = new KeyedListeners<number>();
		const a = vi.fn();
		const b = vi.fn();
		reg.add('s1', a);
		reg.add('s1', b);
		reg.emit('s1', 1);
		expect(a).toHaveBeenCalledTimes(1);
		expect(b).toHaveBeenCalledTimes(1);
	});

	it('the returned fn unsubscribes only that listener', () => {
		const reg = new KeyedListeners<number>();
		const a = vi.fn();
		const b = vi.fn();
		const off = reg.add('s1', a);
		reg.add('s1', b);
		off();
		reg.emit('s1', 1);
		expect(a).not.toHaveBeenCalled();
		expect(b).toHaveBeenCalledTimes(1);
	});

	it('has() reflects registration and drops the key once empty', () => {
		const reg = new KeyedListeners<number>();
		expect(reg.has('s1')).toBe(false);
		const off = reg.add('s1', () => {});
		expect(reg.has('s1')).toBe(true);
		off();
		expect(reg.has('s1')).toBe(false);
	});

	it('emitting an unknown key is a no-op', () => {
		const reg = new KeyedListeners<number>();
		expect(() => reg.emit('nope', 1)).not.toThrow();
	});

	it('a double unsubscribe is harmless', () => {
		const reg = new KeyedListeners<number>();
		const off = reg.add('s1', () => {});
		off();
		expect(() => off()).not.toThrow();
	});
});

describe('BoundedEventBuffer', () => {
	it('appends distinct events in order', () => {
		const buf = new BoundedEventBuffer();
		expect(buf.push(reply('a', 1))).toBe(true);
		expect(buf.push(reply('b', 2))).toBe(true);
		expect(buf.list().map((e) => e.ts)).toEqual([1, 2]);
	});

	it('rejects a byte-identical duplicate', () => {
		const buf = new BoundedEventBuffer();
		expect(buf.push(reply('a', 1))).toBe(true);
		expect(buf.push(reply('a', 1))).toBe(false);
		expect(buf.size).toBe(1);
	});

	it('keeps a repeated body that carries a different ts', () => {
		const buf = new BoundedEventBuffer();
		buf.push(reply('a', 1));
		expect(buf.push(reply('a', 2))).toBe(true);
		expect(buf.size).toBe(2);
	});

	it('caps on event count, evicting oldest first', () => {
		const buf = new BoundedEventBuffer(Number.MAX_SAFE_INTEGER, 3);
		for (let i = 1; i <= 5; i++) buf.push(reply('e', i));
		expect(buf.size).toBe(3);
		expect(buf.list().map((e) => e.ts)).toEqual([3, 4, 5]);
	});

	it('caps on serialized size', () => {
		const buf = new BoundedEventBuffer(200, Number.MAX_SAFE_INTEGER);
		for (let i = 1; i <= 40; i++) buf.push(reply('x'.repeat(20), i));
		expect(buf.size).toBeLessThan(40);
		expect(buf.list().at(-1)?.ts).toBe(40);
	});

	it('always retains the newest event even if it alone exceeds the cap', () => {
		const buf = new BoundedEventBuffer(10, Number.MAX_SAFE_INTEGER);
		buf.push(reply('y'.repeat(500), 1));
		expect(buf.size).toBe(1);
		expect(buf.list()[0].ts).toBe(1);
	});

	it('frees the dedup index on eviction, so an evicted event can reappear', () => {
		const buf = new BoundedEventBuffer(Number.MAX_SAFE_INTEGER, 2);
		buf.push(reply('a', 1));
		buf.push(reply('b', 2));
		buf.push(reply('c', 3));
		expect(buf.push(reply('a', 1))).toBe(true);
	});

	it('clear() empties the buffer and its dedup index', () => {
		const buf = new BoundedEventBuffer();
		buf.push(reply('a', 1));
		buf.clear();
		expect(buf.size).toBe(0);
		expect(buf.list()).toEqual([]);
		expect(buf.push(reply('a', 1))).toBe(true);
	});
});
