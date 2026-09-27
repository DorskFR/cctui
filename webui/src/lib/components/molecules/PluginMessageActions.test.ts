// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import PluginMessageActions from './PluginMessageActions.svelte';
import type { PluginActionButton } from '$lib/plugins/types';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

describe('PluginMessageActions', () => {
	it('renders one button per action and hands the clicked one back', () => {
		const onopen = vi.fn();
		const actions: PluginActionButton[] = [
			{ pluginId: 'review', label: 'Open in Review', icon: 'eye', params: { url: 'u' }, autoOpen: false },
			{ pluginId: 'other', label: 'Other', icon: 'grid', params: {}, autoOpen: false }
		];
		comp = mount(PluginMessageActions, { target: document.body, props: { actions, onopen } });
		flushSync();
		const buttons = document.querySelectorAll<HTMLButtonElement>('[data-journey="plugin-action"]');
		expect([...buttons].map((b) => b.textContent?.trim())).toEqual(['Open in Review', 'Other']);
		expect(buttons[0].dataset.plugin).toBe('review');
		buttons[0].click();
		expect(onopen).toHaveBeenCalledWith(actions[0]);
	});
	it('renders nothing without actions', () => {
		comp = mount(PluginMessageActions, { target: document.body, props: { actions: [], onopen: vi.fn() } });
		flushSync();
		expect(document.querySelector('[data-journey="plugin-actions"]')).toBeNull();
	});
});
