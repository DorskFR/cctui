// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import MetricTile from './MetricTile.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const render = (props: { value: number; label: string; href?: string }) => {
	comp = mount(MetricTile, { target: document.body, props });
	flushSync();
	return document.querySelector('[data-tsu="Card"]') as HTMLElement;
};

describe('MetricTile', () => {
	it('lays a linked tile out as a block so its card fills the grid cell', () => {
		const card = render({ value: 3, label: 'Working', href: '/sessions' });
		expect(card.tagName).toBe('A');
		expect(card.getAttribute('href')).toBe('/sessions');
		expect(card.style.display).toBe('block');
	});

	it('leaves a plain tile as a div', () => {
		const card = render({ value: 3, label: 'Working' });
		expect(card.tagName).toBe('DIV');
		expect(card.getAttribute('style') ?? '').not.toContain('undefined');
	});
});
