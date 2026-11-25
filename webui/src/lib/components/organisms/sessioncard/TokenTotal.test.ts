// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { TokenUsage } from '@bindings/TokenUsage';
import TokenTotal from './TokenTotal.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const usage = (over: Partial<TokenUsage> = {}): TokenUsage => ({
	tokens_in: 1_000_000,
	tokens_out: 58_000,
	cost_usd: 2.38,
	cache_read_tokens: 23_000_000,
	cache_creation_tokens: 942_000,
	...over
});

function render(u: TokenUsage, sum: number | null = null) {
	comp = mount(TokenTotal, { target: document.body, props: { usage: u, sum } });
	flushSync();
}

describe('TokenTotal', () => {
	it('spells the total as tokens, with no glyph', () => {
		render(usage());
		const el = document.querySelector('.tt');
		expect(el?.textContent).toMatch(/^25(\.0)?M tokens$/);
		expect(el?.textContent).not.toMatch(/[Σ↑↓⚡]/);
	});

	it('puts the labelled breakdown in the title', () => {
		render(usage());
		const title = document.querySelector('.tt')?.getAttribute('title') ?? '';
		expect(title).toContain(' in, ');
		expect(title).toContain(' out, ');
		expect(title).toContain(' cached');
	});

	it('uses the subagent rollup when given', () => {
		render(usage(), 3_000);
		expect(document.querySelector('.tt')?.textContent).toMatch(/^3(\.0)?k tokens$/);
	});

	it('says token for one', () => {
		render(
			usage({ tokens_in: 1, tokens_out: 0, cache_read_tokens: 0, cache_creation_tokens: 0 })
		);
		expect(document.querySelector('.tt')?.textContent).toBe('1 token');
	});

	it('renders nothing without usage', () => {
		render(usage({ tokens_in: 0, tokens_out: 0, cache_read_tokens: 0, cache_creation_tokens: 0 }));
		expect(document.querySelector('.tt')).toBeNull();
	});
});
