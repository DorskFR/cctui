import { describe, expect, it } from 'vitest';
import { formatTypedCode } from './device-code.ts';

describe('formatTypedCode', () => {
	it('uppercases and drops separators', () => {
		expect(formatTypedCode('ab cd')).toBe('ABCD');
	});
	it('inserts the hyphen after the fourth character', () => {
		expect(formatTypedCode('abcd1')).toBe('ABCD-1');
		expect(formatTypedCode('abcd-1234')).toBe('ABCD-1234');
	});
	it('stops at eight characters', () => {
		expect(formatTypedCode('abcd12345')).toBe('ABCD-1234');
	});
});
