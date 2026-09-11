import { flushSync, mount, tick, unmount } from 'svelte';
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
	const trigger = document.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]');
	if (!trigger) throw new Error('trigger did not render');
	const hidePopover = vi.fn();
	return {
		onpick,
		trigger,
		hidePopover,
		// happy-dom implements no Popover API: the panel neither fires its own
		// toggle nor answers hidePopover(), so drive both by hand.
		async open() {
			const panel = document.querySelector<HTMLElement & { hidePopover?: () => void }>('[popover]');
			if (!panel) throw new Error('panel did not render');
			panel.hidePopover = hidePopover;
			const toggle = Object.assign(new Event('toggle'), { newState: 'open' });
			panel.dispatchEvent(toggle);
			await tick();
			await tick();
			flushSync();
			const menu = document.querySelector('[role="menu"]');
			if (!menu) throw new Error('menu did not render');
			return menu;
		},
		items: () => [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
	};
}

it('lists stored prompts newest-first and picks one', async () => {
	promptHistory.push('first prompt');
	promptHistory.push('second prompt');
	const menu = setup();
	await menu.open();

	const items = menu.items();
	expect(items.map((b) => b.textContent?.trim())).toEqual(['second prompt', 'first prompt']);

	items[1].click();
	flushSync();
	expect(menu.onpick).toHaveBeenCalledWith('first prompt');
	expect(menu.hidePopover).toHaveBeenCalled();
});

it('previews multiple lines and keeps the full prompt', async () => {
	promptHistory.push('line one\nline two');
	const menu = setup();
	await menu.open();

	const item = menu.items()[0];
	expect(item.textContent?.trim()).toBe('line one\nline two');
	expect(item.title).toBe('line one\nline two');

	item.click();
	flushSync();
	expect(menu.onpick).toHaveBeenCalledWith('line one\nline two');
});

it('renders an empty state and no items with no history', async () => {
	const menu = setup();
	const panel = await menu.open();
	expect(menu.items()).toHaveLength(0);
	expect(panel.textContent?.trim()).not.toBe('');
});

it('clears the stored history', async () => {
	promptHistory.push('doomed');
	const menu = setup();
	await menu.open();
	expect(menu.items()).toHaveLength(1);

	const clear = [...document.querySelectorAll<HTMLButtonElement>('[role="menu"] button')].at(-1);
	clear?.click();
	flushSync();

	expect(promptHistory.get()).toEqual([]);
	expect(menu.items()).toHaveLength(0);
});
