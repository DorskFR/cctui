import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount, type ComponentProps } from 'svelte';
import SessionSectionHeader from './SessionSectionHeader.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

type Props = ComponentProps<typeof SessionSectionHeader>;

function render(over: Partial<Props> = {}) {
	const props = {
		label: 'Completed',
		count: 2,
		sort: 'activity',
		sortDir: 'desc',
		onsort: () => {},
		hidden: false,
		ontogglehidden: () => {},
		...over
	} as Props;
	comp = mount(SessionSectionHeader, { target: document.body, props });
	flushSync();
}

const archiveButton = () =>
	document.querySelector<HTMLButtonElement>('[data-testid="archive-section"]');

describe('SessionSectionHeader', () => {
	it('shows the archive control only when a handler is given, and it names the section', () => {
		render();
		expect(archiveButton()).toBeNull();
		unmount(comp!);
		comp = null;
		document.body.innerHTML = '';

		const onarchive = vi.fn();
		render({ onarchive });
		const btn = archiveButton();
		expect(btn).not.toBeNull();
		expect(btn?.getAttribute('title')).toContain('Completed');
		btn?.click();
		expect(onarchive).toHaveBeenCalledTimes(1);
	});

	it('disables the archive control for an empty section', () => {
		render({ onarchive: () => {}, count: 0 });
		expect(archiveButton()?.disabled).toBe(true);
	});

	it('the eye toggle reports back and reflects the hidden state', () => {
		const ontogglehidden = vi.fn();
		render({ ontogglehidden, hidden: true });
		const eye = [...document.querySelectorAll('button')].find((b) =>
			b.getAttribute('title')?.startsWith('Show')
		);
		expect(eye).toBeDefined();
		eye?.click();
		expect(ontogglehidden).toHaveBeenCalledTimes(1);
	});

	it('the sort trigger names the active field and the direction', () => {
		render({ sort: 'name', sortDir: 'asc' });
		expect(document.body.textContent).toContain('Sort: Name');
		expect(document.querySelector('[aria-label="Ascending"]')).not.toBeNull();
	});

	// happy-dom implements no Popover API, so the panel neither fires its own
	// toggle (leaving the items unrendered) nor answers the hidePopover() that
	// Menu calls before an item's onselect.
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
	}

	it('picking a field from the menu reports that field', async () => {
		const onsort = vi.fn();
		render({ onsort });
		await openMenu();
		const created = [
			...document.querySelectorAll('[role="menuitem"], [role="menuitemcheckbox"]')
		].find((el) => el.textContent?.includes('Created')) as HTMLElement | undefined;
		expect(created).toBeDefined();
		created?.click();
		flushSync();
		expect(onsort).toHaveBeenCalledWith('created');
	});
});
