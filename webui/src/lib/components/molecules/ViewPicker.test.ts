// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import ViewPicker from './ViewPicker.svelte';
import DimensionPicker from './DimensionPicker.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

describe('square toolbar pickers ride the kit Button control/square contract', () => {
	it('ViewPicker is the kit icon toggle — list, cards, tiles, no native select', () => {
		comp = mount(ViewPicker, { target: document.body, props: { view: 'list' } });
		const segments = [...document.querySelectorAll('button')];
		expect(segments).toHaveLength(3);
		expect(document.querySelector('select')).toBeNull();
		expect(segments[0].getAttribute('aria-checked') ?? segments[0].getAttribute('aria-pressed')).toBe(
			'true'
		);
	});

	it('ViewPicker drops the tiles segment where tiles are not offered', () => {
		comp = mount(ViewPicker, { target: document.body, props: { view: 'list', tiles: false } });
		expect(document.querySelectorAll('button')).toHaveLength(2);
	});


	it('ViewPicker in the menu is one full-width kit Button row that cycles the view', () => {
		comp = mount(ViewPicker, { target: document.body, props: { view: 'list', menu: true } });
		const row = document.querySelector('button.btn[data-journey="view"]') as HTMLButtonElement;
		expect(row).not.toBeNull();
		expect(row.className).toContain('btn-block');
		expect(document.querySelectorAll('button')).toHaveLength(1);
		expect(row.textContent).toContain('List');
		row.click();
		flushSync();
		expect(row.textContent).toContain('Cards');
		row.click();
		flushSync();
		expect(row.textContent).toContain('Tiles');
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
