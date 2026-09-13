import { describe, expect, it } from 'vitest';
import { formatElapsed, lastProseLine, toolInvocationSummary, truncate } from './activity';

describe('toolInvocationSummary', () => {
	it('prefixes a shell command with $', () => {
		expect(toolInvocationSummary('Bash', { command: 'cargo test --all', description: 'Run tests' })).toBe(
			'$ cargo test --all'
		);
	});

	it('prefers the most specific key over a generic one', () => {
		expect(toolInvocationSummary('Read', { file_path: '/a/b.rs', prompt: 'ignored' })).toBe('/a/b.rs');
	});

	it('summarizes a URL, a search pattern and a subagent', () => {
		expect(toolInvocationSummary('WebFetch', { url: 'https://example.com/x' })).toBe('https://example.com/x');
		expect(toolInvocationSummary('Grep', { pattern: 'TodoWrite' })).toBe('TodoWrite');
		expect(toolInvocationSummary('Agent', { subagent_type: 'implementer' })).toBe('implementer');
	});

	it('falls back to the first short string for an unknown harness tool', () => {
		expect(toolInvocationSummary('some_codex_tool', { whatever: 'doing a thing' })).toBe('doing a thing');
	});

	it('degrades to an empty string rather than throwing on unusable input', () => {
		for (const bad of [null, undefined, 42, 'str', {}, { n: 1 }, { blank: '   ' }]) {
			expect(toolInvocationSummary('X', bad)).toBe('');
		}
	});

	it('truncates a very long invocation instead of returning it whole', () => {
		const out = toolInvocationSummary('Bash', { command: 'x'.repeat(500) });
		expect(out.length).toBeLessThanOrEqual(140);
		expect(out.endsWith('…')).toBe(true);
	});

	it('collapses newlines so the summary stays one line', () => {
		expect(toolInvocationSummary('Bash', { command: 'a\n  b\nc' })).toBe('$ a b c');
	});
});

describe('truncate', () => {
	it('leaves a short string alone', () => {
		expect(truncate('short')).toBe('short');
	});
	it('caps at the requested max with an ellipsis', () => {
		expect(truncate('abcdefghij', 5)).toBe('abcd…');
	});
});

describe('formatElapsed', () => {
	it('formats seconds, minutes and hours the way the harness footer does', () => {
		expect(formatElapsed(0)).toBe('0s');
		expect(formatElapsed(45_000)).toBe('45s');
		expect(formatElapsed(170_000)).toBe('2m 50s');
		expect(formatElapsed(3_780_000)).toBe('1h 3m');
	});
	it('never renders a negative duration from clock skew', () => {
		expect(formatElapsed(-5000)).toBe('0s');
	});
});

describe('lastProseLine', () => {
	it('takes the last non-empty line', () => {
		expect(lastProseLine('first para\n\nTest passes. Committing.\n\n')).toBe('Test passes. Committing.');
	});
	it('is null for blank prose', () => {
		expect(lastProseLine('   \n\n')).toBeNull();
	});
});
