// @vitest-environment happy-dom
import { mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import CountChip from './CountChip.svelte';

let target: HTMLElement | null = null;
let app: Record<string, unknown> | null = null;

afterEach(() => {
	if (app) unmount(app);
	target?.remove();
	app = null;
	target = null;
});

const render = (props: { count: number; tone?: 'neutral' | 'active'; label?: string }) => {
	target = document.createElement('div');
	document.body.appendChild(target);
	app = mount(CountChip, { target, props });
	return target;
};

describe('CountChip', () => {
	it('shows the count in the neutral tone by default', () => {
		const chip = render({ count: 5 }).querySelector('.count');
		expect(chip?.textContent).toBe('5');
		expect(chip?.classList.contains('active')).toBe(false);
	});

	it('caps the visible number at 99+', () => {
		expect(render({ count: 140 }).querySelector('.count')?.textContent).toBe('99+');
	});

	it('renders nothing for zero', () => {
		expect(render({ count: 0 }).querySelector('.count')).toBeNull();
	});

	it('names the count for assistive tech when given a label', () => {
		const chip = render({ count: 5, label: '5 unread', tone: 'active' }).querySelector('.count');
		expect(chip?.getAttribute('aria-label')).toBe('5 unread');
		expect(chip?.getAttribute('role')).toBe('img');
		expect(chip?.classList.contains('active')).toBe(true);
	});
});
