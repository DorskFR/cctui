import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import SubagentBadge from './SubagentBadge.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const badge = () => {
	const el = document.querySelector('button');
	if (!el) throw new Error('badge button not found');
	return el;
};
const open = (props: Record<string, unknown>) => {
	comp = mount(SubagentBadge, {
		target: document.body,
		props: { count: 5, running: 2, open: false, label: 'subagents', ontoggle: () => {}, ...props }
	});
};

describe('SubagentBadge', () => {
	it('shows the agent type ahead of the count for a typed group', () => {
		open({ type: 'general-purpose', label: 'general-purpose subagents' });
		expect(badge().textContent?.replace(/\s+/g, ' ').trim()).toBe('general-purpose 5');
		expect(document.querySelector('.type')?.textContent).toBe('general-purpose');
	});

	it('stays a bare count chip when the group has no single agent type', () => {
		open({ type: null });
		expect(badge().textContent?.trim()).toBe('5');
		expect(document.querySelector('.type')).toBeNull();
	});

	it('defaults to the bare count chip when no type is passed at all', () => {
		open({});
		expect(badge().textContent?.trim()).toBe('5');
		expect(document.querySelector('.type')).toBeNull();
	});

	it('keeps the group label and expand state on the accessible name', () => {
		open({ type: 'Explore', label: 'Explore subagents' });
		const name = badge().getAttribute('aria-label') ?? '';
		expect(name).toContain('Explore subagents');
		expect(badge().getAttribute('aria-expanded')).toBe('false');
	});

	it('toggles the group and stops the click reaching the row handler', () => {
		// Asserted on the event, not via a listener on an ancestor: the row's
		// handler is a Svelte `onclick` sharing one delegated root listener
		// with this one, and a native ancestor listener fires earlier still,
		// during the real bubble — neither can observe stopPropagation here.
		const ontoggle = vi.fn();
		open({ type: 'Explore', ontoggle });
		const click = new MouseEvent('click', { bubbles: true, cancelable: true });
		const stopped = vi.spyOn(click, 'stopPropagation');
		badge().dispatchEvent(click);
		expect(ontoggle).toHaveBeenCalledTimes(1);
		expect(stopped).toHaveBeenCalled();
	});

	it('does not let a press on the badge start the row swipe', () => {
		// Pointer events are not delegated, so the row wrapper's
		// onpointerdown={swipe.start} really is an ancestor listener.
		const onrow = vi.fn();
		const row = document.createElement('div');
		row.addEventListener('pointerdown', onrow);
		document.body.append(row);
		comp = mount(SubagentBadge, {
			target: row,
			props: { count: 5, running: 2, open: false, label: 'subagents', ontoggle: () => {} }
		});
		badge().dispatchEvent(new Event('pointerdown', { bubbles: true, cancelable: true }));
		expect(onrow).not.toHaveBeenCalled();
	});
});
