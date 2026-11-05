// @vitest-environment happy-dom
import { mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import PageHead from './PageHead.svelte';

let target: HTMLElement | null = null;
let app: Record<string, unknown> | null = null;

afterEach(() => {
	if (app) unmount(app);
	target?.remove();
	app = null;
	target = null;
});

const render = (props: { title: string; subtitle?: string }) => {
	target = document.createElement('div');
	document.body.appendChild(target);
	app = mount(PageHead, { target, props });
	return target;
};

describe('PageHead', () => {
	it('renders the title as the page h1', () => {
		expect(render({ title: 'Bookmarks' }).querySelector('h1')?.textContent).toBe('Bookmarks');
	});

	it('renders the subtitle under the title', () => {
		const titles = render({ title: 'Bookmarks', subtitle: 'Saved replies' }).querySelector('.titles');
		expect(titles?.children).toHaveLength(2);
		expect(titles?.children[1]?.textContent).toBe('Saved replies');
	});

	it('omits the subtitle when none is given', () => {
		expect(render({ title: 'Bookmarks' }).querySelector('.titles')?.children).toHaveLength(1);
	});
});
