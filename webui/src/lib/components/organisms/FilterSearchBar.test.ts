import { mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { FilterSearchBar, type Schema } from '@dorsk/tsumikit';

const schema: Schema = {
	fields: [
		{ name: 'status', label: 'Status', type: 'enum' },
		{ name: 'model', label: 'Model', type: 'string' },
		{ name: 'machine', label: 'Machine', type: 'string' }
	]
};

let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
	vi.restoreAllMocks();
});

async function renderSearchBar() {
	component = mount(FilterSearchBar, {
		target: document.body,
		props: { schema, showChips: false }
	});
	await tick();
	const input = document.querySelector<HTMLInputElement>('.fsb__input');
	expect(input).not.toBeNull();
	input!.focus();
	await vi.waitFor(() => expect(document.querySelectorAll('.fsb__opt')).toHaveLength(3));
	return input!;
}

function selectedOption() {
	return document.querySelector<HTMLButtonElement>('.fsb__opt[aria-selected="true"]');
}

describe('cctui FilterSearchBar interaction baseline', () => {
	it('wraps keyboard navigation and scrolls the active option into view', async () => {
		const scrollIntoView = vi.fn();
		Object.defineProperty(window.HTMLElement.prototype, 'scrollIntoView', {
			configurable: true,
			value: scrollIntoView
		});
		const input = await renderSearchBar();

		input.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
		await tick();

		expect(selectedOption()?.textContent).toContain('Machine');
		expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest' });

		input.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
		await tick();
		expect(selectedOption()?.textContent).toContain('Status');
	});

	it('tracks pointer hover without forcing keyboard scrolling', async () => {
		const scrollIntoView = vi.fn();
		Object.defineProperty(window.HTMLElement.prototype, 'scrollIntoView', {
			configurable: true,
			value: scrollIntoView
		});
		await renderSearchBar();
		const options = document.querySelectorAll<HTMLButtonElement>('.fsb__opt');

		options[1].dispatchEvent(new MouseEvent('mouseenter'));
		await tick();

		expect(selectedOption()?.textContent).toContain('Model');
		expect(scrollIntoView).not.toHaveBeenCalled();
	});
});
