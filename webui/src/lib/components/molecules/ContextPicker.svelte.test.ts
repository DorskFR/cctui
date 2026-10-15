// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import ContextPicker from './ContextPicker.svelte';
import type { Pickable } from './context.logic';

const item = (name: string, title: string, kind = 'memory'): Pickable => ({
	id: `id-${name}`,
	kind,
	name,
	title,
	tags: []
});
const items = [item('alpha', 'Alpha'), item('beta', 'Beta'), item('gamma', 'Gamma')];

let component: ReturnType<typeof mount> | undefined;
afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
});

function render(initial: string[] = [], locked: string[] = []) {
	const state = $state({ value: initial });
	component = mount(ContextPicker, {
		target: document.body,
		props: {
			items,
			locked,
			label: 'Context',
			get value() {
				return state.value;
			},
			set value(v: string[]) {
				state.value = v;
			}
		}
	});
	flushSync();
	return state;
}

const input = () => {
	const el = document.querySelector('input');
	if (!el) throw new Error('no input');
	return el;
};
function key(k: string) {
	input().dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true }));
	flushSync();
}
const chips = () => document.querySelectorAll('[data-chip]').length;

describe('ContextPicker keyboard multi-pick', () => {
	it('picks several items without closing, and Backspace drops the last chip', () => {
		const state = render();
		input().focus();
		input().dispatchEvent(new FocusEvent('focus'));
		flushSync();
		expect(document.querySelector('[role="listbox"]')).not.toBeNull();

		key('ArrowDown');
		key('Enter');
		key('ArrowDown');
		key('Enter');
		expect(state.value).toEqual(['beta', 'gamma']);
		expect(chips()).toBe(2);
		expect(document.querySelector('[role="listbox"]')).not.toBeNull();

		key('Backspace');
		expect(state.value).toEqual(['beta']);
		expect(chips()).toBe(1);
	});

	it('marks picked rows and unpicks on a second Enter', () => {
		const state = render(['alpha']);
		input().dispatchEvent(new FocusEvent('focus'));
		flushSync();
		expect(document.querySelector('[data-option="alpha"]')?.textContent).toContain('✓');
		key('Enter');
		expect(state.value).toEqual([]);
	});

	it('never removes a locked chip', () => {
		const state = render(['beta'], ['alpha']);
		expect(chips()).toBe(2);
		key('Backspace');
		expect(state.value).toEqual([]);
		key('Backspace');
		expect(state.value).toEqual([]);
		expect(chips()).toBe(1);
	});
});
