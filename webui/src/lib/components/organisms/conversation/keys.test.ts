import { describe, it, expect } from 'vitest';
import { assignLineKeys } from './format';
import type { Line } from './types';

const toolCall = (ts: number, text: string): Line => ({ role: 'tool', ts, tool: 'exec_command', text });

describe('assignLineKeys', () => {
	it('gives distinct keys to colliding tool calls (same ts/role/24-char prefix)', () => {
		const prefix = '{\n  "cmd": "git grep -n -I ';
		const lines = [toolCall(1783793006401, prefix + 'foo'), toolCall(1783793006401, prefix + 'bar')];
		assignLineKeys(lines);
		expect(lines[0].key).not.toBe(lines[1].key);
		const keys = new Set(lines.map((l) => l.key));
		expect(keys.size).toBe(2);
	});

	it('produces stable keys across two builds of the same event array', () => {
		const build = (): Line[] => [
			{ role: 'user', ts: 1, text: 'hi' },
			toolCall(2, 'a'),
			toolCall(2, 'a'),
			{ role: 'assistant', ts: 3, text: 'ok' }
		];
		const a = assignLineKeys(build()).map((l) => l.key);
		const b = assignLineKeys(build()).map((l) => l.key);
		expect(a).toEqual(b);
	});

	it('leaves a lone line with its bare content key (no suffix)', () => {
		const lines = [toolCall(5, 'unique')];
		assignLineKeys(lines);
		expect(lines[0].key).toBe('5|tool|unique');
	});
});
