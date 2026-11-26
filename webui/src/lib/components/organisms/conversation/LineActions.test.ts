// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
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

const byLabel = (root: HTMLElement, label: string) =>
	[...root.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label) ?? null;

describe('line action cluster', () => {
	it('uses no raw buttons of its own', () => {
		expect(actionsSource).not.toContain('<button');
	});

	it('shows every action inline, with no ⋯ menu', async () => {
		const el = await render({ onquote: vi.fn(), onspeak: vi.fn() });
		for (const label of ['Pin message', 'Save as image', 'Copy as Markdown', 'Quote reply', 'Read aloud', 'Save this message to bookmarks'])
			expect(byLabel(el, label), label).not.toBeNull();
		expect(el.querySelector('[aria-haspopup="menu"]')).toBeNull();
		expect(actionsSource).not.toContain('<Menu');
	});

	it('reflects the pin and bookmark state as pressed buttons', async () => {
		const el = await render({ pinned: true, bookmarked: true });
		expect(byLabel(el, 'Unpin message')?.getAttribute('aria-pressed')).toBe('true');
		expect(byLabel(el, 'Save this message to bookmarks')?.getAttribute('aria-pressed')).toBe('true');
	});

	it('drops pin and bookmark when the line offers neither', async () => {
		const el = await render({ pinnable: false, onbookmark: undefined });
		expect([...el.querySelectorAll('button')].map((b) => b.getAttribute('aria-label'))).toEqual([
			'Save as image',
			'Copy as Markdown'
		]);
	});

	it('saves the image from the line the cluster sits in', async () => {
		const onsaveimage = vi.fn();
		const el = await render({ onsaveimage });
		byLabel(el, 'Save as image')?.click();
		expect(onsaveimage).toHaveBeenCalledWith(el, ln);
	});

	it('turns read aloud into a pressed stop while the line is read', async () => {
		const el = await render({ onspeak: vi.fn(), speaking: true });
		expect(byLabel(el, 'Read aloud')).toBeNull();
		expect(el.querySelector('button[aria-pressed="true"]')).not.toBeNull();
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
