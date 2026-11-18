import { describe, expect, it } from 'vitest';
import { formatStat } from './statText';

describe('formatStat', () => {
	it('gives a token count its unit word, compacted without a trailing .0', () => {
		expect(formatStat(25_000_000, 'tokens')).toBe('25M tokens');
		expect(formatStat(1_234_567, 'tokens')).toBe('1.2M tokens');
		expect(formatStat(4_200, 'tokens')).toBe('4.2k tokens');
	});

	it('uses the singular for one', () => {
		expect(formatStat(1, 'tokens')).toBe('1 token');
		expect(formatStat(1, 'tools')).toBe('1 tool');
		expect(formatStat(1, 'files')).toBe('1 file');
		expect(formatStat(1, 'messages')).toBe('1 message');
	});

	it('uses the plural otherwise', () => {
		expect(formatStat(31, 'tools')).toBe('31 tools');
		expect(formatStat(0, 'files')).toBe('0 files');
		expect(formatStat(12, 'messages')).toBe('12 messages');
	});

	it('spells the full number when not compact', () => {
		expect(formatStat(25_000_000, 'tokens', false)).toBe('25,000,000 tokens');
	});

	it('labels the token kinds', () => {
		expect(formatStat(1_500_000, 'input')).toBe('1.5M input');
		expect(formatStat(300_000, 'output')).toBe('300k output');
		expect(formatStat(4_000_000, 'cache')).toBe('4M cached');
	});

	it('formats cost as dollars', () => {
		expect(formatStat(2.38, 'cost')).toBe('$2.38');
		expect(formatStat(0, 'cost')).toBe('$0.00');
	});

	it('treats missing or negative values as zero', () => {
		expect(formatStat(Number.NaN, 'tools')).toBe('0 tools');
		expect(formatStat(-3, 'tokens')).toBe('0 tokens');
	});
});
