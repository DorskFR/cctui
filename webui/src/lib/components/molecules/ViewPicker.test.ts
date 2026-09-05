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
	it('ViewPicker toolbar trigger is a default-tier square Button hosting the ghost select', () => {
		comp = mount(ViewPicker, {
			target: document.body,
			props: { cardView: false, kanban: false }
		});
		const btn = document.querySelector('button.btn');
		expect(btn?.classList.contains('btn-control')).toBe(false);
		expect(btn?.classList.contains('btn-square')).toBe(true);
		expect(btn?.querySelector('select')).not.toBeNull();
		expect(document.querySelector('.btn-control-square')).toBeNull();
	});

	it('ViewPicker offers exactly list, cards and kanban', () => {
		comp = mount(ViewPicker, {
			target: document.body,
			props: { cardView: true, kanban: false }
		});
		const select = document.querySelector('select') as HTMLSelectElement;
		expect([...select.options].map((o) => o.value)).toEqual(['list', 'card', 'kanban']);
		expect(document.querySelector('button.btn')?.getAttribute('title')).toContain('Cards');
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
