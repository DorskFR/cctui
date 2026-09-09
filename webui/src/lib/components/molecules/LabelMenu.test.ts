import { flushSync, mount, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import type { Label } from '@bindings/Label';
import LabelMenu from './LabelMenu.svelte';
import LabelBadge from './LabelBadge.svelte';

let component: ReturnType<typeof mount>;
afterEach(async () => {
	if (component) await unmount(component);
	document.body.replaceChildren();
});

function element<T extends Element>(selector: string): T {
	const node = document.querySelector<T>(selector);
	if (!node) throw new Error(`Missing element: ${selector}`);
	return node;
}

function setup(names: string[], options: { cap?: number; busy?: boolean } = {}) {
	const onToggle = vi.fn();
	const onCreate = vi.fn();
	const labels = names.map((name, id) => ({ id: String(id), name, color: '' }) as Label);
	component = mount(LabelMenu, {
		target: document.body,
		props: { labels, selectedIds: new Set<string>(), onToggle, onCreate, ...options }
	});
	flushSync();
	const input = element<HTMLInputElement>('input');
	return {
		onToggle, onCreate, input,
		type(value: string) {
			input.value = value;
			input.dispatchEvent(new Event('input', { bubbles: true }));
			flushSync();
		},
		submit() {
			element<HTMLFormElement>('form').dispatchEvent(new Event('submit', { cancelable: true }));
			flushSync();
		}
	};
}

it('focuses the search field on mount', () => {
	const { input } = setup(['bandstream']);
	expect(document.activeElement).toBe(input);
});

it('selects the sole partial match instead of creating the query', () => {
	const menu = setup(['bandstream', 'cockpit']);
	menu.type(' BAND ');
	menu.submit();
	expect(menu.onToggle).toHaveBeenCalledWith(expect.objectContaining({ name: 'bandstream' }));
	expect(menu.onCreate).not.toHaveBeenCalled();
});

it('still creates the partial name when Create is clicked', () => {
	const menu = setup(['bandstream']);
	menu.type('band');
	element<HTMLButtonElement>('.create').click();
	expect(menu.onCreate).toHaveBeenCalledWith('band');
	expect(menu.onToggle).not.toHaveBeenCalled();
});

it.each([
	[['bandstream', 'bandcamp'], 'band'],
	[['cockpit'], 'band']
])('keeps creation with zero or multiple matches: %j', (names, query) => {
	const menu = setup(names, { cap: 1 });
	menu.type(query);
	menu.submit();
	expect(menu.onCreate).toHaveBeenCalledWith(query);
	expect(menu.onToggle).not.toHaveBeenCalled();
});

it('keeps exact-match priority among multiple matches', () => {
	const menu = setup(['band', 'bandstream']);
	menu.type('band');
	menu.submit();
	expect(menu.onToggle).toHaveBeenCalledWith(expect.objectContaining({ name: 'band' }));
	expect(menu.onCreate).not.toHaveBeenCalled();
});

it.each(['', '   '])('does nothing for an empty query: %j', (query) => {
	const menu = setup(['bandstream']);
	menu.type(query);
	menu.submit();
	expect(menu.onToggle).not.toHaveBeenCalled();
	expect(menu.onCreate).not.toHaveBeenCalled();
});

it('does not select while busy', () => {
	const menu = setup(['bandstream'], { busy: true });
	menu.type('band');
	menu.submit();
	expect(menu.onToggle).not.toHaveBeenCalled();
});

it('focuses the session picker on first opening and reopening', async () => {
	component = mount(LabelBadge, {
		target: document.body,
		props: { labels: [], allLabels: [], editable: true }
	});
	flushSync();
	const panel = element<HTMLElement>('[popover]');
	const trigger = element<HTMLButtonElement>('button');
	for (const state of ['open', 'closed', 'open']) {
		trigger.focus();
		panel.dispatchEvent(Object.assign(new Event('toggle'), { newState: state }));
		flushSync();
		await Promise.resolve();
		if (state === 'open') expect(document.activeElement).toBe(document.querySelector('input'));
	}
});
