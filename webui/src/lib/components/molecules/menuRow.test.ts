// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import DimensionPicker from './DimensionPicker.svelte';
import LabelFilter from './LabelFilter.svelte';
import ViewPicker from './ViewPicker.svelte';
import { MENU_ROW, MENU_ROW_ICON } from './menuRow';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const label = { id: 'l-1', name: 'bug', color: '#888', created_at: '2026-01-01T00:00:00Z' };

const ROWS = [
	{ name: 'view', Component: ViewPicker, props: { view: 'list', menu: true }, selector: 'button[data-journey="view"]' },
	{
		name: 'dimension',
		Component: DimensionPicker,
		props: { menu: true, kind: 'group', value: 'status', onchange: () => {} },
		selector: '.dim-picker.menu-row'
	},
	{
		name: 'label filter',
		Component: LabelFilter,
		props: { menu: true, labels: [label], selected: new Set<string>() },
		selector: '.label-filter.menu-row button'
	}
] as const;

const declared = (style: string) =>
	style
		.split(';')
		.map((d) => d.trim().replace(/\s*:\s*/, ': ').replace(/\s+/g, ' '))
		.filter(Boolean)
		.sort();

function render(entry: (typeof ROWS)[number]) {
	comp = mount(entry.Component as Parameters<typeof mount>[0], {
		target: document.body,
		props: entry.props as Record<string, unknown>
	});
	flushSync();
	const el = document.querySelector<HTMLElement>(entry.selector);
	if (!el) throw new Error(`${entry.name} row did not render`);
	return el;
}

function reset() {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
}

describe('sessions ⋯ menu rows share one row chrome', () => {
	it.each(ROWS.map((r) => [r.name, r] as const))('the %s row carries it', (_name, entry) => {
		const el = render(entry);
		expect(declared(el.getAttribute('style') ?? '')).toEqual(declared(MENU_ROW));
	});

	it('gives every row a fixed leading icon column holding its glyph', () => {
		for (const entry of ROWS) {
			const el = render(entry);
			const box = [...el.querySelectorAll<HTMLElement>('span')].find(
				(s) =>
					declared(s.getAttribute('style') ?? '').join('; ') === declared(MENU_ROW_ICON).join('; ')
			);
			expect(box, `${entry.name} leading icon column`).toBeDefined();
			expect(box?.querySelector('svg')).not.toBeNull();
			reset();
		}
	});
});
