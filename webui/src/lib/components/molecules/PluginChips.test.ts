// @vitest-environment happy-dom
import { flushSync, mount, unmount, type ComponentProps } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import PluginChips from './PluginChips.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function render(props: ComponentProps<typeof PluginChips>) {
	comp = mount(PluginChips, { target: document.body, props });
	flushSync();
}

function chips(): string[] {
	return [...document.querySelectorAll('[data-journey="plugin-chip"]')].map(
		(e) => e.textContent?.trim() ?? ''
	);
}

describe('PluginChips', () => {
	it('renders one chip per slot that has a renderer', () => {
		render({ metadata: { plugins: { youtrack: { issue: 'CCT-910' }, mystery: { a: 1 } } } });
		expect(chips()).toEqual(['CCT-910']);
	});

	it('links the chip when the slot carries a url, and hovers summary and state', () => {
		render({
			metadata: {
				plugins: {
					youtrack: {
						issue: 'CCT-910',
						summary: 'the slot',
						state: 'Open',
						url: 'https://youtrack.example/issue/CCT-910'
					}
				}
			}
		});
		const a = document.querySelector<HTMLAnchorElement>('a[data-journey="plugin-chip"]');
		expect(a?.getAttribute('href')).toBe('https://youtrack.example/issue/CCT-910');
		expect(a?.title).toBe('CCT-910\nthe slot\nOpen');
	});

	it('renders nothing at all when no slot is set', () => {
		render({ metadata: { draft: {} } });
		expect(document.querySelector('[data-journey="plugin-chips"]')).toBeNull();
	});

	it('has no empty-state button, even when suggestable', () => {
		render({ metadata: {}, suggestable: true, onlink: vi.fn() });
		expect(document.querySelector('[data-journey="plugin-chips"]')).toBeNull();
		expect(document.querySelector('button')).toBeNull();
	});

	it('offers a detected id as a one-click link while nothing is stored', () => {
		const onlink = vi.fn();
		render({ metadata: {}, suggestable: true, detected: 'CCT-910', onlink });
		const suggest = document.querySelector('[data-journey="plugin-chip-suggest"]');
		expect(suggest?.textContent?.trim()).toBe('+ CCT-910');
		suggest?.querySelector('button')?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
		flushSync();
		expect(onlink).toHaveBeenCalledWith('CCT-910');
	});

	it('does not offer a detected id once an issue is linked', () => {
		render({
			metadata: { plugins: { youtrack: { issue: 'CCT-1' } } },
			suggestable: true,
			detected: 'CCT-910',
			onlink: vi.fn()
		});
		expect(document.querySelector('[data-journey="plugin-chip-suggest"]')).toBeNull();
		expect(chips()).toEqual(['CCT-1']);
	});

	it('does not offer a detected id when not suggestable, e.g. on the card', () => {
		render({ metadata: {}, detected: 'CCT-910' });
		expect(document.querySelector('[data-journey="plugin-chip-suggest"]')).toBeNull();
		expect(document.querySelector('[data-journey="plugin-chips"]')).toBeNull();
	});
});
