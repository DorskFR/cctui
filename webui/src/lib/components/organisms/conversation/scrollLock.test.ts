// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import Host from './ScrollLock.host.test.svelte';
import { lockDocumentScroll } from './scrollLock';

const html = () => document.documentElement;

afterEach(() => {
	html().style.overflowY = '';
});

describe('lockDocumentScroll', () => {
	it('hides the document scrollbar and restores nothing set before', () => {
		const unlock = lockDocumentScroll();
		expect(html().style.overflowY).toBe('hidden');
		unlock();
		expect(html().style.overflowY).toBe('');
	});

	it('restores the inline value it replaced', () => {
		html().style.overflowY = 'scroll';
		const unlock = lockDocumentScroll();
		expect(html().style.overflowY).toBe('hidden');
		unlock();
		expect(html().style.overflowY).toBe('scroll');
	});

	it('nests without leaking hidden', () => {
		const outer = lockDocumentScroll();
		const inner = lockDocumentScroll();
		inner();
		expect(html().style.overflowY).toBe('hidden');
		outer();
		expect(html().style.overflowY).toBe('');
	});

	it('locks while a component is mounted and unlocks on destroy', () => {
		const comp = mount(Host, { target: document.body });
		flushSync();
		expect(html().style.overflowY).toBe('hidden');
		unmount(comp);
		flushSync();
		expect(html().style.overflowY).toBe('');
	});
});
