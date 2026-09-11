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

	it('toggles the group without letting the click reach the row beneath', () => {
		const ontoggle = vi.fn();
		const onrow = vi.fn();
		document.body.addEventListener('click', onrow);
		open({ type: 'Explore', ontoggle });
		badge().click();
		document.body.removeEventListener('click', onrow);
		expect(ontoggle).toHaveBeenCalledTimes(1);
		expect(onrow).not.toHaveBeenCalled();
	});
});
