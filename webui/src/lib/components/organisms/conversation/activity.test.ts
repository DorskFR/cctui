import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import {
	agentActivityAt,
	formatElapsed,
	lastProseLine,
	toolInvocationSummary,
	truncate,
	workingAfter
} from './activity';

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

describe('workingAfter', () => {
	const text = (meta: boolean, kind: string | null = null) =>
		({ type: 'text', content: 'x', meta, kind, ts: 1 }) as AgentEvent;
	const run = (events: AgentEvent[]) => events.reduce(workingAfter, false);

	it('ends the turn on turn_end', () => {
		expect(run([text(false), { type: 'turn_end', ts: 2 }])).toBe(false);
	});

	it('keeps a finished turn finished through the lines that trail turn_end', () => {
		expect(
			run([
				text(false),
				{ type: 'turn_end', ts: 2 },
				text(true, 'turn_annotation'),
				text(true, 'system_marker'),
				text(true, 'queue_op'),
				{ type: 'turn_summary', detail: 'done', status_category: 'done', needs_action: false, ts: 3 } as AgentEvent,
				{ type: 'heartbeat', ts: 4 } as AgentEvent
			])
		).toBe(false);
	});

	it('starts a turn on agent output', () => {
		expect(run([text(false)])).toBe(true);
		expect(run([{ type: 'tool_call', tool: 'Bash', input: {}, ts: 1 } as unknown as AgentEvent])).toBe(true);
	});
});

describe('agentActivityAt', () => {
	const text = (meta: boolean, ts: number) => ({ type: 'text', content: 'x', meta, ts }) as AgentEvent;
	const run = (events: AgentEvent[]) => events.reduce<number | null>(agentActivityAt, null);

	it('keeps the newest prose, tool or user-message time', () => {
		expect(
			run([
				text(false, 5),
				{ type: 'tool_call', tool: 'Bash', input: {}, ts: 9 } as unknown as AgentEvent,
				text(false, 7)
			])
		).toBe(9);
	});

	it('ignores meta lines, summaries and heartbeats', () => {
		expect(
			run([
				text(false, 5),
				text(true, 50),
				{ type: 'heartbeat', ts: 60 } as AgentEvent,
				{ type: 'turn_end', ts: 70 } as AgentEvent
			])
		).toBe(5);
		expect(run([{ type: 'heartbeat', ts: 60 } as AgentEvent])).toBeNull();
	});
});
