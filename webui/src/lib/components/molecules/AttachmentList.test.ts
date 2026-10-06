// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, type ComponentProps } from 'svelte';
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
	const att = { files, remove: onremove, images: { pending: [] } } as unknown as ComponentProps<
		typeof AttachmentList
	>['att'];
	comp = mount(AttachmentList, { target: document.body, props: { att } });
	const list = document.querySelector('[data-tsu="AttachmentList"]');
	if (!list) throw new Error('list not found');
	return list;
}

describe('AttachmentList', () => {
	it('renders auto tiles with no numbers', () => {
		const list = render();
		expect(list.classList.contains('auto')).toBe(true);
		expect(list.querySelectorAll('.num').length).toBe(0);
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
