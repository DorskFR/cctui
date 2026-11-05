// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import SessionMissing from './SessionMissing.svelte';
import { ARCHIVED_STATUS, archivedSearchHref } from './missing';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

describe('missing session', () => {
	it('links to the archived search on the sessions list', () => {
		const url = new URL(archivedSearchHref(), 'http://x');
		expect(url.pathname).toBe('/sessions');
		expect(url.searchParams.get('status')).toBe(ARCHIVED_STATUS);
		expect([...url.searchParams.keys()]).toEqual(['status']);
	});

	it('says what happened, names the id, and offers both ways out', () => {
		const onclose = vi.fn();
		comp = mount(SessionMissing, { target: document.body, props: { id: 'abc-123', onclose } });
		flushSync();
		const root = document.querySelector('[data-testid="session-missing"]') as HTMLElement;
		expect(root.textContent).toContain('archived or deleted');
		expect(root.textContent).toContain('abc-123');
		const link = root.querySelector('a[href]') as HTMLAnchorElement;
		expect(link.getAttribute('href')).toBe(archivedSearchHref());
		const back = [...root.querySelectorAll('button')].find((b) => b.textContent?.includes('Back to sessions'));
		back?.click();
		expect(onclose).toHaveBeenCalledOnce();
	});
});
