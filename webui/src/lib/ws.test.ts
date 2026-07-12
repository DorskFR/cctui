import { describe, expect, it } from 'vitest';
import { decodeBase64 } from './ws.svelte';

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
