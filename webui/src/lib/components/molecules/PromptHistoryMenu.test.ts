import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { promptHistory } from '$lib/drafts';
import PromptHistoryMenu from './PromptHistoryMenu.svelte';

let component: ReturnType<typeof mount>;

beforeEach(() => {
	localStorage.clear();
});
afterEach(async () => {
	if (component) await unmount(component);
	document.body.replaceChildren();
});

function setup() {
	const onpick = vi.fn();
	component = mount(PromptHistoryMenu, { target: document.body, props: { onpick } });
	flushSync();
	const trigger = document.querySelector<HTMLButtonElement>('button[aria-haspopup="true"]');
	if (!trigger) throw new Error('trigger did not render');
	return {
		onpick,
		trigger,
		open() {
			trigger.click();
			flushSync();
			const menu = document.querySelector('[role="menu"]');
			if (!menu) throw new Error('menu did not render');
			return menu;
		},
		items: () => [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
	};
}

it('lists stored prompts newest-first and picks one', () => {
	promptHistory.push('first prompt');
	promptHistory.push('second prompt');
	const menu = setup();
	menu.open();

	const items = menu.items();
	expect(items.map((b) => b.textContent?.trim())).toEqual(['second prompt', 'first prompt']);

	items[1].click();
	flushSync();
	expect(menu.onpick).toHaveBeenCalledWith('first prompt');
	expect(document.querySelector('[role="menu"]')).toBeNull();
});

it('shows a single-line preview but keeps the full prompt', () => {
	promptHistory.push('line one\nline two');
	const menu = setup();
	menu.open();

	const item = menu.items()[0];
	expect(item.textContent?.trim()).toBe('line one');
	expect(item.title).toBe('line one\nline two');

	item.click();
	flushSync();
	expect(menu.onpick).toHaveBeenCalledWith('line one\nline two');
});

it('renders an empty state and no items with no history', () => {
	const menu = setup();
	const panel = menu.open();
	expect(menu.items()).toHaveLength(0);
	expect(panel.textContent?.trim()).not.toBe('');
});

it('clears the stored history', () => {
	promptHistory.push('doomed');
	const menu = setup();
	menu.open();
	expect(menu.items()).toHaveLength(1);

	const clear = [...document.querySelectorAll<HTMLButtonElement>('[role="menu"] button')].at(-1);
	clear?.click();
	flushSync();

	expect(promptHistory.get()).toEqual([]);
	expect(menu.items()).toHaveLength(0);
});
