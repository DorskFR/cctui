// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
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

// happy-dom has no Popover API: open the panel by hand.
async function openMenu() {
	const panel = document.querySelector('[popover]') as
		| (HTMLElement & { hidePopover?: () => void })
		| null;
	expect(panel).not.toBeNull();
	if (panel && typeof panel.hidePopover !== 'function') panel.hidePopover = () => {};
	const toggle = new Event('toggle') as Event & { newState: string };
	toggle.newState = 'open';
	panel?.dispatchEvent(toggle);
	await tick();
	return [...document.querySelectorAll<HTMLElement>('[role^="menuitem"]')];
}

const item = (rows: HTMLElement[], label: string) =>
	rows.find((r) => r.textContent?.includes(label)) ?? null;

describe('line action buttons', () => {
	it('uses no raw buttons of its own', () => {
		expect(actionsSource).not.toContain('<button');
	});

	it('keeps only quote reply and the more menu on the line', async () => {
		const el = await render({ onquote: vi.fn() });
		const direct = [...el.querySelectorAll('button')].filter((b) => !b.closest('[popover]'));
		expect(direct.length).toBe(2);
		expect(el.querySelector('.quote-btn')).not.toBeNull();
	});

	it('puts pin, copy, image and bookmark under the more menu', async () => {
		await render();
		const rows = await openMenu();
		expect(item(rows, 'Pin message')).not.toBeNull();
		expect(item(rows, 'Copy as Markdown')).not.toBeNull();
		expect(item(rows, 'Save as image')).not.toBeNull();
		expect(item(rows, 'Save this message to bookmarks')).not.toBeNull();
	});

	it('marks a saved bookmark as checked', async () => {
		await render({ bookmarked: true });
		const b = item(await openMenu(), 'Save this message to bookmarks');
		expect(b?.getAttribute('aria-checked')).toBe('true');
	});

	it('offers unpin on a pinned line', async () => {
		await render({ pinned: true });
		expect(item(await openMenu(), 'Unpin message')).not.toBeNull();
	});

	it('hides the pin when the line is not pinnable', async () => {
		await render({ pinnable: false });
		const rows = await openMenu();
		expect(item(rows, 'Pin message')).toBeNull();
		expect(rows.length).toBe(3);
	});

	it('saves the enclosing line node as an image', async () => {
		const onsaveimage = vi.fn();
		const host = document.createElement('div');
		host.className = 'line';
		document.body.appendChild(host);
		comp = mount(LineActions, {
			target: host,
			props: { ln, pinnable: false, onsaveimage, oncopymarkdown: vi.fn() }
		});
		await flush();
		item(await openMenu(), 'Save as image')?.click();
		expect(onsaveimage).toHaveBeenCalledWith(host, ln);
	});

	it('draws no glyph-only bookmark emoji', () => {
		expect(actionsSource).not.toContain('◈');
	});
});

describe('read aloud', () => {
	it('offers read aloud only when a speak handler is given', async () => {
		expect(item(await openAfter(render()), 'Read aloud')).toBeNull();
		if (comp) unmount(comp);
		comp = null;
		document.body.innerHTML = '';
		expect(item(await openAfter(render({ onspeak: vi.fn() })), 'Read aloud')).not.toBeNull();
	});

	it('keeps a visible stop button while speaking', async () => {
		const onspeak = vi.fn();
		const el = await render({ onspeak, speaking: true });
		const stop = [...el.querySelectorAll('button')].find(
			(b) => b.getAttribute('aria-label') === 'Stop reading' && !b.closest('[popover]')
		);
		expect(stop).toBeDefined();
		stop?.click();
		expect(onspeak).toHaveBeenCalledWith(ln);
	});
});

async function openAfter(rendered: Promise<HTMLElement>) {
	await rendered;
	return openMenu();
}
