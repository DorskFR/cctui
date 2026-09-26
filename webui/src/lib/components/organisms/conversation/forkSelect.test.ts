// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { ForkSelection, forkRange, forkableIds } from './forkSelect.svelte';
import type { Line } from './types';

const line = (role: Line['role'], messageId?: string) =>
	({ role, messageId, ts: 0, text: '' }) as unknown as Line;

const lines = [
	line('user'),
	line('assistant', 'a1'),
	line('tool'),
	line('assistant', 'a2'),
	line('assistant'),
	line('assistant', 'a3')
];

describe('forkableIds', () => {
	it('keeps assistant lines with a messageId, in render order', () => {
		expect(forkableIds(lines)).toEqual(['a1', 'a2', 'a3']);
	});
});

describe('forkRange', () => {
	it('is empty when nothing selected is an anchor', () => {
		expect(forkRange(['a1', 'a2'], [])).toEqual([]);
		expect(forkRange(['a1', 'a2'], ['zz'])).toEqual([]);
	});

	it('forks just the one checked message', () => {
		expect(forkRange(['a1', 'a2', 'a3'], ['a2'])).toEqual(['a2']);
	});

	it('spans the outermost checked messages inclusively, whatever the check order', () => {
		expect(forkRange(['a1', 'a2', 'a3'], ['a3', 'a1'])).toEqual(['a1', 'a2', 'a3']);
	});
});

describe('ForkSelection', () => {
	it('toggles membership and clears on exit', () => {
		const sel = new ForkSelection();
		sel.toggleMode();
		expect(sel.active).toBe(true);
		sel.toggle('a1');
		sel.toggle('a3');
		sel.toggle('a1');
		expect([...sel.selected]).toEqual(['a3']);
		expect(sel.range(lines)).toEqual(['a3']);
		sel.toggleMode();
		expect(sel.active).toBe(false);
		expect(sel.selected.size).toBe(0);
	});
});
