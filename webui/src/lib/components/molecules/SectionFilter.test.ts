// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { SECTIONS, type Section } from '../../../routes/sessions/sessions.logic';
import SectionFilter from './SectionFilter.svelte';

let component: ReturnType<typeof mount>;

afterEach(async () => {
	if (component) await unmount(component);
	document.body.replaceChildren();
});

function setup(sections: Section[]) {
	component = mount(SectionFilter, {
		target: document.body,
		props: { sections: new Set(sections) }
	});
	flushSync();
	const trigger = document.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]');
	if (!trigger) throw new Error('trigger did not render');
	const hidePopover = vi.fn();
	return {
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
		},
		options: () =>
			[...document.querySelectorAll<HTMLButtonElement>('[role="menuitemcheckbox"]')].map((el) => ({
				el,
				key: el.getAttribute('data-journey-key'),
				checked: el.getAttribute('aria-checked') === 'true'
			})),
		option(key: Section) {
			const found = this.options().find((o) => o.key === key);
			if (!found) throw new Error(`no option for ${key}`);
			return found;
		}
	};
}

it('renders one checkbox item per section, carrying the journey hooks', async () => {
	const menu = setup(['live']);
	await menu.open();

	const options = menu.options();
	expect(options.map((o) => o.key)).toEqual(SECTIONS.map((s) => s.value));
	for (const o of options) expect(o.el.getAttribute('data-journey')).toBe('option');
	expect(options.filter((o) => o.checked).map((o) => o.key)).toEqual(['live']);
});

it('keeps the menu open while toggling several sections', async () => {
	const menu = setup(['live']);
	await menu.open();

	menu.option('archived').el.click();
	flushSync();
	expect(menu.option('archived').checked).toBe(true);
	expect(menu.hidePopover).not.toHaveBeenCalled();

	menu.option('starred').el.click();
	flushSync();
	expect(menu.option('starred').checked).toBe(true);
	expect(menu.option('archived').checked).toBe(true);
	expect(menu.hidePopover).not.toHaveBeenCalled();
});

it('never lets the last enabled section be turned off', async () => {
	const menu = setup(['live']);
	await menu.open();

	menu.option('live').el.click();
	flushSync();
	expect(menu.option('live').checked).toBe(true);
});

it('counts the enabled sections on the trigger while any section is off', () => {
	setup(['live']);
	expect(document.querySelector('[data-tsu="Badge"]')?.textContent?.trim()).toBe('1');
});

it('drops the count once every section is enabled', () => {
	setup(SECTIONS.map((s) => s.value));
	expect(document.querySelector('[data-tsu="Badge"]')).toBeNull();
});
