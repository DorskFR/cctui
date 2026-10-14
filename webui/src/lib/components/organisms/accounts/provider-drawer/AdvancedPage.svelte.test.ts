// @vitest-environment happy-dom
import { describe, it, expect, afterEach, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import AdvancedPage from './AdvancedPage.svelte';

vi.mock('$lib/queries', () => ({ useUsageProbes: () => ({}) }));

let host: HTMLElement | null = null;
let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	host?.remove();
	comp = null;
	host = null;
});

function render(settings: Record<string, unknown> = {}) {
	host = document.createElement('div');
	document.body.appendChild(host);
	const props = $state({
		rawJson: true,
		catalogReady: true,
		settableKeys: new Set(['editorMode']),
		blockedKeys: new Set(['forceLoginMethod']),
		settings
	});
	comp = mount(AdvancedPage, { target: host, props });
	flushSync();
	return { el: host, props };
}

function merge(el: HTMLElement, text: string) {
	const box = el.querySelector('textarea')!;
	box.value = text;
	box.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
	[...el.querySelectorAll('button')].find((b) => b.textContent?.includes('Merge'))!.click();
	flushSync();
}

const badges = (el: HTMLElement) =>
	[...el.querySelectorAll('.loose [data-tsu="Badge"], .loose .badge')].map((b) => b.textContent?.trim());

describe('AdvancedPage raw JSON', () => {
	it('merges a key the schema does not know and tags it out of schema', () => {
		const { el, props } = render();
		merge(el, '{ "idleCompaction": false }');
		expect(props.settings).toEqual({ idleCompaction: false });
		expect(el.querySelector('.loose')?.textContent).toContain('idleCompaction: false');
		expect(badges(el)).toEqual(['out of schema']);
	});

	it('does not tag a catalogued key', () => {
		const { el } = render({ editorMode: 'vim' });
		expect(el.querySelector('.loose')?.textContent).toContain('editorMode: "vim"');
		expect(badges(el)).toEqual([]);
	});

	it('rejects MANAGED/SYSTEM keys without merging anything', () => {
		const { el, props } = render();
		merge(el, '{ "forceLoginMethod": "console", "idleCompaction": false }');
		expect(props.settings).toEqual({});
		expect(el.textContent).toContain('Not settable per-account (MANAGED/SYSTEM): forceLoginMethod');
	});
});
