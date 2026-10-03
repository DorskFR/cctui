// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import AttachmentList from './AttachmentList.svelte';

const previewFile = vi.fn();
vi.mock('$lib/fileviewer', () => ({ previewFile: (f: File) => previewFile(f) }));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	previewFile.mockClear();
	document.body.innerHTML = '';
});

const files = [
	new File(['x'], 'a.txt', { type: 'text/plain' }),
	new File(['y'], 'shot.png', { type: 'image/png' })
];

function render(onremove = (_name: string) => {}) {
	URL.createObjectURL = vi.fn(() => 'blob:thumb');
	URL.revokeObjectURL = vi.fn();
	comp = mount(AttachmentList, { target: document.body, props: { files, onremove } });
	const list = document.querySelector('[data-tsu="AttachmentList"]');
	if (!list) throw new Error('list not found');
	return list;
}

describe('AttachmentList', () => {
	it('renders auto tiles, numbered to match the [#N] prompt markers', () => {
		const list = render();
		expect(list.classList.contains('auto')).toBe(true);
		expect([...list.querySelectorAll('li.chip .num')].map((n) => n.textContent)).toEqual(['1', '2']);
		expect(list.querySelectorAll('li.tile').length).toBe(2);
	});

	it('previews a file from its chip name and removes by name', () => {
		const removed: string[] = [];
		const list = render((name) => removed.push(name));
		const chips = list.querySelectorAll('li.chip');
		(chips[1].querySelector('button.open') as HTMLButtonElement).click();
		expect(previewFile).toHaveBeenCalledWith(files[1]);
		(chips[0].querySelector('button[aria-label="Remove"]') as HTMLButtonElement).click();
		expect(removed).toEqual(['a.txt']);
	});
});
