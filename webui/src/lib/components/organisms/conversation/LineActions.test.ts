// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import LineActions from './LineActions.svelte';
import actionsSource from './LineActions.svelte?raw';
import type { Line } from './types';

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const ln: Line = { role: 'assistant', ts: 1_700_000_000_000, html: '<p>hi</p>', seq: 3 };

async function render(props: Record<string, unknown> = {}) {
	comp = mount(LineActions, {
		target: document.body,
		props: {
			ln,
			pinnable: true,
			onpin: vi.fn(),
			onsaveimage: vi.fn(),
			oncopymarkdown: vi.fn(),
			onbookmark: vi.fn(),
			...props
		}
	});
	await flush();
	return document.querySelector('.line-actions') as HTMLElement;
}

const byLabel = (root: HTMLElement, label: string) =>
	[...root.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label) ?? null;

describe('line action buttons', () => {
	it('uses no raw buttons of its own', () => {
		expect(actionsSource).not.toContain('<button');
	});

	it('exposes the bookmark toggle state via aria-pressed', async () => {
		const off = await render({ bookmarked: false });
		const b = byLabel(off, 'Save this message to bookmarks');
		expect(b).not.toBeNull();
		expect(b?.getAttribute('aria-pressed')).toBe('false');
	});

	it('marks a saved bookmark as pressed', async () => {
		const on = await render({ bookmarked: true });
		const b = byLabel(on, 'Save this message to bookmarks');
		expect(b?.getAttribute('aria-pressed')).toBe('true');
	});

	it('exposes the pin toggle state via aria-pressed', async () => {
		const el = await render({ pinned: true });
		const pressed = [...el.querySelectorAll('button')].filter(
			(b) => b.getAttribute('aria-pressed') === 'true'
		);
		expect(pressed.length).toBeGreaterThan(0);
	});

	it('hides the pin when the line is not pinnable', async () => {
		const el = await render({ pinnable: false });
		expect(el.querySelectorAll('button').length).toBe(3);
	});
});
