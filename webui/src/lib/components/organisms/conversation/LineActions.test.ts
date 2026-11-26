// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import LineActions from './LineActions.svelte';
import actionsSource from './LineActions.svelte?raw';
import lineSource from './ConversationLine.svelte?raw';
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

async function openMenu() {
	const panel = [...document.querySelectorAll('[popover]')].at(-1) as HTMLElement;
	panel.dispatchEvent(Object.assign(new Event('toggle'), { newState: 'open' }));
	flushSync();
	await Promise.resolve();
	flushSync();
}

const rows = () =>
	[...document.querySelectorAll('[role="menuitem"], [role="menuitemcheckbox"]')] as HTMLElement[];
const row = (text: string) => rows().find((r) => r.textContent?.includes(text)) ?? null;
const byLabel = (root: HTMLElement, label: string) =>
	[...root.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label) ?? null;

describe('line action cluster', () => {
	it('uses no raw buttons of its own', () => {
		expect(actionsSource).not.toContain('<button');
	});

	it('shows only quote reply and the ⋯ menu inline', async () => {
		const el = await render({ onquote: vi.fn(), onspeak: vi.fn() });
		expect(byLabel(el, 'Quote reply')).not.toBeNull();
		expect(byLabel(el, 'Message actions')).not.toBeNull();
		expect(byLabel(el, 'Copy as Markdown')).toBeNull();
		expect(byLabel(el, 'Save as image')).toBeNull();
		expect(byLabel(el, 'Read aloud')).toBeNull();
	});

	it('lists pin, read aloud, copy, save and bookmark under ⋯', async () => {
		await render({ onspeak: vi.fn() });
		await openMenu();
		for (const label of ['Pin message', 'Read aloud', 'Copy as Markdown', 'Save as image', 'Save this message to bookmarks'])
			expect(row(label), label).not.toBeNull();
	});

	it('reflects the pin and bookmark state as checked rows', async () => {
		await render({ pinned: true, bookmarked: true });
		await openMenu();
		expect(row('Unpin message')?.getAttribute('aria-checked')).toBe('true');
		expect(row('Save this message to bookmarks')?.getAttribute('aria-checked')).toBe('true');
	});

	it('drops the pin row when the line is not pinnable', async () => {
		await render({ pinnable: false, onbookmark: undefined });
		await openMenu();
		expect(rows().map((r) => r.textContent?.trim())).toEqual(['Copy as Markdown', 'Save as image']);
	});

	it('saves the image from the line the cluster sits in', async () => {
		const onsaveimage = vi.fn();
		const el = await render({ onsaveimage });
		await openMenu();
		row('Save as image')?.click();
		expect(onsaveimage).toHaveBeenCalledWith(el, ln);
	});

	it('keeps a stop control inline while the line is being read', async () => {
		const el = await render({ onspeak: vi.fn(), speaking: true });
		expect(el.querySelector('button[aria-pressed="true"]')).not.toBeNull();
		await openMenu();
		expect(row('Read aloud')).toBeNull();
	});
});

describe('line action reveal', () => {
	it('fades the cluster until the line is hovered or focused, on hover pointers', () => {
		const css = lineSource.slice(lineSource.indexOf('<style>'));
		expect(css).toMatch(/@media \(hover: hover\) \{\s*\.line:not\(:hover, :focus-within\) \.acts:not\(\.keep\) \{\s*opacity: 0;/);
	});

	it('shows it on touch only for the tapped line', () => {
		const css = lineSource.slice(lineSource.indexOf('<style>'));
		expect(css).toMatch(/@media \(hover: none\) \{\s*\.line:not\(:focus-within\) \.acts:not\(\.keep\) \{\s*display: none;/);
		expect(lineSource).toContain('tabindex="-1"');
	});
});
