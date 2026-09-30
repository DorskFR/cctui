import { describe, expect, it } from 'vitest';
import { detectIssueId, extractIssueId, parseIssueEntry } from './issueId';

describe('extractIssueId', () => {
	it('finds an id anywhere in the text', () => {
		expect(extractIssueId('CCT-123 fix the thing')).toBe('CCT-123');
		expect(extractIssueId('fix the thing (CCT-123)')).toBe('CCT-123');
		expect(extractIssueId('refs/heads/CCT-910')).toBe('CCT-910');
	});

	it('takes the first of several', () => {
		expect(extractIssueId('CCT-1 then AB-22')).toBe('CCT-1');
	});

	it('rejects anything that is not PROJECT-<digits>', () => {
		for (const s of ['cct-123', 'Cct-123', 'CCT123', 'CCT-', '-123', 'xCCT-123', 'CCT-12a', 'CCT_1-2']) {
			expect(extractIssueId(s), s).toBeNull();
		}
	});

	it('is null for no text', () => {
		expect(extractIssueId(null)).toBeNull();
		expect(extractIssueId(undefined)).toBeNull();
		expect(extractIssueId('')).toBeNull();
	});
});

describe('detectIssueId', () => {
	it('prefers the prompt, then the name, then the branch', () => {
		expect(detectIssueId('CCT-1 do it', 'CCT-2 session', 'feat/CCT-3')).toBe('CCT-1');
		expect(detectIssueId('do it', 'CCT-2 session', 'feat/CCT-3')).toBe('CCT-2');
		expect(detectIssueId('do it', 'a session', 'feat/CCT-3')).toBe('CCT-3');
	});

	it('is null when no source carries an id', () => {
		expect(detectIssueId('do it', 'a session', 'lane/q3-plugin-slot')).toBeNull();
		expect(detectIssueId(null, undefined, '')).toBeNull();
	});
});

describe('parseIssueEntry', () => {
	it('accepts a bare id, upper-casing what the user typed', () => {
		expect(parseIssueEntry(' cct-910 ')).toEqual({ issue: 'CCT-910' });
		expect(parseIssueEntry('CCT-910')).toEqual({ issue: 'CCT-910' });
	});

	it('accepts a pasted issue url and keeps it as the chip link', () => {
		expect(parseIssueEntry('https://youtrack.example/issue/CCT-910')).toEqual({
			issue: 'CCT-910',
			url: 'https://youtrack.example/issue/CCT-910'
		});
	});

	it('rejects empty input and anything without an id', () => {
		expect(parseIssueEntry('')).toBeNull();
		expect(parseIssueEntry('   ')).toBeNull();
		expect(parseIssueEntry('not a ticket')).toBeNull();
		expect(parseIssueEntry('https://youtrack.example/dashboard')).toBeNull();
	});
});
