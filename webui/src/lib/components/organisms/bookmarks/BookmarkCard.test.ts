// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import type { Bookmark } from '@bindings/Bookmark';
import BookmarkCard from './BookmarkCard.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const bm = (over: Partial<Bookmark> = {}): Bookmark => ({
	id: 'b1',
	session_id: 's1',
	seq: 3,
	message_id: null,
	title: 'Wrap-up',
	body: '## Done\n\n- shipped',
	role: 'assistant',
	session_name: 'nightly',
	note: null,
	message_ts: '2026-09-09T10:00:00Z',
	created_at: '2026-09-09T10:00:01Z',
	...over
});

async function render(bookmark: Bookmark) {
	comp = mount(BookmarkCard, {
		target: document.body,
		props: { bookmark, onopen: vi.fn(), oncopy: vi.fn(), onedit: vi.fn(), ondelete: vi.fn() }
	});
	await new Promise((r) => setTimeout(r, 0));
	return document.querySelector('.card') as HTMLElement;
}

describe('BookmarkCard', () => {
	it('renders an assistant bookmark in the drawer’s assistant bubble', async () => {
		const card = await render(bm());
		const bubble = card.querySelector('.bubble');
		expect(bubble?.classList.contains('assistant')).toBe(true);
		expect(bubble?.tagName).toBe('DIV');
		expect(card.querySelector('.lmeta')?.textContent).toContain('assistant');
		expect(card.querySelector('.dot')).toBeNull();
	});

	it('renders a tool bookmark as highlighted code, not a markdown fence', async () => {
		const card = await render(
			bm({ role: 'tool', body: '**Tool · Bash**\n\n```sh\nls -la\n```' })
		);
		const bubble = card.querySelector('.bubble');
		expect(bubble?.tagName).toBe('PRE');
		expect(bubble?.classList.contains('tool')).toBe(true);
		expect(bubble?.textContent).toContain('ls -la');
		expect(bubble?.textContent).not.toContain('```');
		expect(card.querySelector('.tool-name')?.textContent).toBe('Bash');
	});

	it('offers Expand only when the collapsed body overflows', async () => {
		const card = await render(bm());
		expect(card.querySelector('footer')?.textContent).not.toContain('Show more');
	});
});
