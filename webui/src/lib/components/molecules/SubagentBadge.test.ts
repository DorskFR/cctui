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
	it('renders one bare numeric count chip and no other text', () => {
		open({});
		expect(badge().textContent?.replace(/\s+/g, ' ').trim()).toBe('5');
	});

	it('keeps the group label and expand state on the accessible name', () => {
		open({ label: 'Explore subagents' });
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
		open({ ontoggle });
		const click = new MouseEvent('click', { bubbles: true, cancelable: true });
		const stopped = vi.spyOn(click, 'stopPropagation');
		badge().dispatchEvent(click);
		expect(ontoggle).toHaveBeenCalledTimes(1);
		expect(stopped).toHaveBeenCalled();
	});

	it('stops a press on the badge from arming the row swipe', () => {
		// pointerdown is on Svelte's delegated list, so this handler and the
		// row wrapper's onpointerdown={swipe.start} share one root listener
		// that honours stopPropagation. Asserted on the event for the same
		// reason as the click above.
		open({});
		const press = new Event('pointerdown', { bubbles: true, cancelable: true });
		const stopped = vi.spyOn(press, 'stopPropagation');
		badge().dispatchEvent(press);
		expect(stopped).toHaveBeenCalled();
	});
});
