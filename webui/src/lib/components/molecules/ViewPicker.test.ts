import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import ViewPicker from './ViewPicker.svelte';
import DimensionPicker from './DimensionPicker.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

describe('square toolbar pickers ride the kit Button control/square contract', () => {
	it('ViewPicker is the kit icon toggle — two segments, no native select', () => {
		comp = mount(ViewPicker, { target: document.body, props: { cardView: false } });
		const segments = [...document.querySelectorAll('button')];
		expect(segments).toHaveLength(2);
		expect(document.querySelector('select')).toBeNull();
		// list is selected while cardView is false
		expect(segments[0].getAttribute('aria-checked') ?? segments[0].getAttribute('aria-pressed')).toBe(
			'true'
		);
	});


	it('menu rows stay plain full-width rows without a Button', () => {
		comp = mount(DimensionPicker, {
			target: document.body,
			props: { menu: true, kind: 'group', value: 'status', onchange: () => {} }
		});
		expect(document.querySelector('.dim-picker.menu-row')).not.toBeNull();
		expect(document.querySelector('button')).toBeNull();
	});

	it('DimensionPicker tints an active dimension through a style prop, not a global class', () => {
		comp = mount(DimensionPicker, {
			target: document.body,
			props: { kind: 'color', value: 'machine', onchange: () => {} }
		});
		expect(document.querySelector('button.btn')?.getAttribute('style')).toContain('var(--accent)');
	});
});
