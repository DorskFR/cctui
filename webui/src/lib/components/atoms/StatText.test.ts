// @vitest-environment happy-dom
import { mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import StatText from './StatText.svelte';

let target: HTMLElement | null = null;
let app: Record<string, unknown> | null = null;

afterEach(() => {
	if (app) unmount(app);
	target?.remove();
	app = null;
	target = null;
});

const render = (props: { value: number; kind: 'tokens' | 'tools' | 'cost'; compact?: boolean }) => {
	target = document.createElement('div');
	document.body.appendChild(target);
	app = mount(StatText, { target, props });
	return target.querySelector('.stat');
};

describe('StatText', () => {
	it('renders the value with its unit word in the numeric face', () => {
		const el = render({ value: 25_000_000, kind: 'tokens' });
		expect(el?.textContent).toBe('25M tokens');
		expect(el?.classList.contains('num')).toBe(true);
	});

	it('puts the exact figure in the title when compacted', () => {
		expect(render({ value: 25_000_000, kind: 'tokens' })?.getAttribute('title')).toBe('25,000,000 tokens');
	});

	it('has no title when nothing was compacted', () => {
		expect(render({ value: 31, kind: 'tools' })?.hasAttribute('title')).toBe(false);
	});

	it('has no title on a cost', () => {
		expect(render({ value: 2.38, kind: 'cost' })?.hasAttribute('title')).toBe(false);
	});
});
