import { afterEach, describe, expect, it } from 'vitest';
import { type ComponentProps, createRawSnippet, mount, unmount } from 'svelte';
import AccessTable from './AccessTable.svelte';

interface Row {
	id: string;
	revoked_at: string | null;
}

const row = createRawSnippet<[Row]>((r) => ({
	render: () => `<span class="cell">${r().id}</span>`
}));

const columns = [
	{ key: 'a', label: 'Key', width: 'minmax(0, 1fr)' },
	{ key: 'b', width: '56px' }
];

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const render = (rows: Row[], empty = 'Nothing here') =>
	mount(AccessTable, {
		target: document.body,
		props: {
			columns,
			rows,
			rowKey: (r: Row) => r.id,
			row,
			empty,
			dim: (r: Row) => !!r.revoked_at
		} as unknown as ComponentProps<typeof AccessTable>
	});

describe('AccessTable', () => {
	it('lays every row out on the column template and dims revoked ones', () => {
		comp = render([
			{ id: 'a', revoked_at: null },
			{ id: 'b', revoked_at: '2026-01-01' }
		]);
		const table = document.querySelector<HTMLElement>('.tbl');
		expect(table?.style.getPropertyValue('--cols')).toBe('minmax(0, 1fr) 56px');
		const rows = [...document.querySelectorAll('.trow')];
		expect(rows.map((r) => r.textContent)).toEqual(['a', 'b']);
		expect(rows[0].className).not.toContain('dim');
		expect(rows[1].className).toContain('dim');
	});

	it('keeps the column header out of the accessibility tree', () => {
		comp = render([{ id: 'a', revoked_at: null }]);
		const header = document.querySelector('.hrow');
		expect(header?.getAttribute('aria-hidden')).toBe('true');
		expect(header?.textContent).toBe('Key');
	});

	it('shows the empty copy instead of a body when there are no rows', () => {
		comp = render([]);
		expect(document.querySelector('.tbody')).toBeNull();
		expect(document.querySelector('.msg')?.textContent).toContain('Nothing here');
	});
});
