// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import PluginChips from './PluginChips.svelte';
import { setYouTrackLookup } from '$lib/plugins/youtrackLookup';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	setYouTrackLookup(null);
});

function render(props: Record<string, unknown>) {
	comp = mount(PluginChips, { target: document.body, props });
	flushSync();
}

function chips(): string[] {
	return [...document.querySelectorAll('[data-journey="plugin-chip"]')].map(
		(e) => e.textContent?.trim() ?? ''
	);
}

function click(journey: string) {
	const host = document.querySelector<HTMLElement>(`[data-journey="${journey}"]`);
	expect(host, journey).not.toBeNull();
	(host?.querySelector('button') ?? host)?.dispatchEvent(
		new MouseEvent('click', { bubbles: true })
	);
	flushSync();
}

function type(value: string) {
	const input = document.querySelector<HTMLInputElement>(
		'[data-journey="plugin-chip-entry"] input'
	);
	expect(input).not.toBeNull();
	if (!input) return;
	input.value = value;
	input.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
}

describe('PluginChips', () => {
	it('renders one chip per slot that has a renderer', () => {
		render({ metadata: { plugins: { youtrack: { issue: 'CCT-910' }, mystery: { a: 1 } } } });
		expect(chips()).toEqual(['CCT-910']);
	});

	it('renders nothing read-only when there are no slots', () => {
		render({ metadata: { draft: {} } });
		expect(document.querySelector('[data-journey="plugin-chips"]')).toBeNull();
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

	it('offers no editor unless editable', () => {
		render({ metadata: { plugins: { youtrack: { issue: 'CCT-910' } } } });
		expect(document.querySelector('[data-journey="plugin-chip-edit"]')).toBeNull();
	});

	it('sets the issue by hand, with no lookup installed', async () => {
		const onset = vi.fn();
		render({ metadata: {}, editable: true, onset });
		click('plugin-chip-edit');
		type('cct-910');
		click('plugin-chip-apply');
		await vi.waitFor(() => expect(onset).toHaveBeenCalled());
		expect(onset).toHaveBeenCalledWith('youtrack', { issue: 'CCT-910' });
	});

	it('keeps a pasted url as the chip link', async () => {
		const onset = vi.fn();
		render({ metadata: {}, editable: true, onset });
		click('plugin-chip-edit');
		type('https://youtrack.example/issue/CCT-910');
		click('plugin-chip-apply');
		await vi.waitFor(() => expect(onset).toHaveBeenCalled());
		expect(onset).toHaveBeenCalledWith('youtrack', {
			issue: 'CCT-910',
			url: 'https://youtrack.example/issue/CCT-910'
		});
	});

	it('enriches the slot through the lookup seam once a connector installs one', async () => {
		const onset = vi.fn();
		setYouTrackLookup(async (issue) => ({ issue, summary: 'looked up', state: 'Open' }));
		render({ metadata: {}, editable: true, onset });
		click('plugin-chip-edit');
		type('CCT-910');
		click('plugin-chip-apply');
		await vi.waitFor(() => expect(onset).toHaveBeenCalled());
		expect(onset).toHaveBeenCalledWith('youtrack', {
			issue: 'CCT-910',
			summary: 'looked up',
			state: 'Open'
		});
	});

	it('clears the slot when the field is emptied', () => {
		const onset = vi.fn();
		render({ metadata: { plugins: { youtrack: { issue: 'CCT-910' } } }, editable: true, onset });
		click('plugin-chip-edit');
		type('');
		click('plugin-chip-apply');
		expect(onset).toHaveBeenCalledWith('youtrack', null);
	});

	it('refuses input that carries no issue id and stays open', () => {
		const onset = vi.fn();
		render({ metadata: {}, editable: true, onset });
		click('plugin-chip-edit');
		type('nonsense');
		click('plugin-chip-apply');
		expect(onset).not.toHaveBeenCalled();
		expect(document.querySelector('[data-journey="plugin-chip-entry"]')).not.toBeNull();
	});

	it('offers a detected id while the slot is empty, and links it in one click', async () => {
		const onset = vi.fn();
		render({ metadata: {}, editable: true, detected: 'CCT-910', onset });
		expect(
			document.querySelector('[data-journey="plugin-chip-suggest"]')?.textContent?.trim()
		).toBe('+ CCT-910');
		click('plugin-chip-suggest');
		await vi.waitFor(() => expect(onset).toHaveBeenCalledWith('youtrack', { issue: 'CCT-910' }));
	});

	it('stops offering a detected id once the slot is filled', () => {
		render({
			metadata: { plugins: { youtrack: { issue: 'CCT-1' } } },
			editable: true,
			detected: 'CCT-910',
			onset: vi.fn()
		});
		expect(document.querySelector('[data-journey="plugin-chip-suggest"]')).toBeNull();
		expect(chips()).toEqual(['CCT-1']);
	});
});
