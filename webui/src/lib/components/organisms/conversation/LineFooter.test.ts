// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import LineFooter from './LineFooter.svelte';
import type { Line } from './types';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const render = (ln: Line) => {
	comp = mount(LineFooter, { target: document.body, props: { ln } });
	flushSync();
	return document.body.textContent ?? '';
};

describe('LineFooter', () => {
	it('spells the per-turn stats in words, no glyphs', () => {
		const text = render({
			role: 'assistant',
			ts: 1,
			durationMs: 146_000,
			usage: {
				tokens_in: 2,
				tokens_out: 757,
				cost_usd: 0,
				cache_read_tokens: 134_000,
				cache_creation_tokens: 0
			}
		});
		expect(text).toContain('2m 26s');
		expect(text).toContain('2 input');
		expect(text).toContain('757 output');
		expect(text).toMatch(/134k cached/i);
		expect(text).not.toMatch(/[⏱↑↓⚡Σ]/);
	});

	it('names the stop hook and counts edited files in words', () => {
		const text = render({ role: 'assistant', ts: 1, stopHook: 'lint', fileHistory: ['a.ts', 'b.ts'] });
		expect(text).toContain('lint');
		expect(text).toContain('2 files');
		expect(text).not.toMatch(/[⏹✎]/);
	});
});
