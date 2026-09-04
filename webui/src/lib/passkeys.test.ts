import { describe, expect, it } from 'vitest';
import { fromBase64Url, toBase64Url } from './passkeys';

/** The whole passkey exchange rests on these two: the server speaks base64url
 *  and `navigator.credentials` speaks bytes, so a padding or alphabet slip
 *  turns into an unexplained "assertion rejected" and nothing else. */
describe('base64url <-> bytes', () => {
	const bytes = (...v: number[]) => new Uint8Array(v).buffer;

	it('uses the url alphabet and drops padding', () => {
		// 0xfb 0xff encodes to "+/8" in standard base64 — the two characters
		// that must become "-" and "_".
		expect(toBase64Url(bytes(0xfb, 0xff))).toBe('-_8');
		expect(toBase64Url(bytes(1))).toBe('AQ');
		expect(toBase64Url(new ArrayBuffer(0))).toBe('');
	});

	it('decodes every unpadded length', () => {
		expect(new Uint8Array(fromBase64Url('-_8'))).toEqual(new Uint8Array([0xfb, 0xff]));
		expect(new Uint8Array(fromBase64Url('AQ'))).toEqual(new Uint8Array([1]));
		expect(new Uint8Array(fromBase64Url(''))).toEqual(new Uint8Array([]));
	});

	it('round-trips every byte value', () => {
		const all = new Uint8Array(256).map((_, i) => i);
		expect(new Uint8Array(fromBase64Url(toBase64Url(all.buffer)))).toEqual(all);
	});

	it('accepts a padded string too, since not every server strips it', () => {
		expect(new Uint8Array(fromBase64Url('AQ=='))).toEqual(new Uint8Array([1]));
	});
});
