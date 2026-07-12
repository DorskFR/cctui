import { describe, it, expect } from 'vitest';
import { stampTurns } from './format';
import type { Line } from './types';

const user = (ts: number): Line => ({ role: 'user', ts, text: `u${ts}` });
const assistant = (ts: number): Line => ({ role: 'assistant', ts, text: `a${ts}` });
const system = (ts: number): Line => ({ role: 'system', ts, text: `s${ts}` });
const tool = (ts: number): Line => ({ role: 'tool', ts, tool: 'Bash' });
const result = (ts: number): Line => ({ role: 'result', ts, tool: 'Bash' });
const reset = (ts: number): Line => ({ role: 'reset', ts, text: 'reset' });
const compact = (ts: number): Line => ({ role: 'compact', ts, text: 'compact' });

const turns = (lines: Line[]) => lines.filter((l) => l.role === 'assistant').map((l) => l.turn);

describe('stampTurns (CCT-552)', () => {
	it('numbers each user→assistant cycle from 1', () => {
		const lines = [user(1), assistant(2), user(3), assistant(4), user(5), assistant(6)];
		stampTurns(lines);
		expect(turns(lines)).toEqual([1, 2, 3]);
	});

	it('shares one turn across tool-interleaved assistant lines', () => {
		const lines = [user(1), assistant(2), tool(3), result(4), assistant(5), user(6), assistant(7)];
		stampTurns(lines);
		expect(turns(lines)).toEqual([1, 1, 2]);
	});

	it('only stamps assistant lines', () => {
		const lines = [user(1), tool(2), result(3), assistant(4)];
		stampTurns(lines);
		expect(lines[0].turn).toBeUndefined();
		expect(lines[1].turn).toBeUndefined();
		expect(lines[2].turn).toBeUndefined();
		expect(lines[3].turn).toBe(1);
	});

	it('opens turn 1 for a leading assistant with no prior prompt', () => {
		const lines = [assistant(1), user(2), assistant(3)];
		stampTurns(lines);
		expect(turns(lines)).toEqual([1, 2]);
	});

	it('counts system prompts as new turns', () => {
		const lines = [user(1), assistant(2), system(3), assistant(4)];
		stampTurns(lines);
		expect(turns(lines)).toEqual([1, 2]);
	});

	it('resets the counter on /clear but not /compact', () => {
		const cleared = [user(1), assistant(2), reset(3), user(4), assistant(5)];
		stampTurns(cleared);
		expect(turns(cleared)).toEqual([1, 1]);

		const compacted = [user(1), assistant(2), compact(3), user(4), assistant(5)];
		stampTurns(compacted);
		expect(turns(compacted)).toEqual([1, 2]);
	});
});
