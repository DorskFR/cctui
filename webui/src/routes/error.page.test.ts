// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const state = vi.hoisted(() => ({
	page: {
		status: 404,
		error: null as { message: string } | null,
		url: new URL('http://x.test/does-not-exist')
	}
}));
vi.mock('$app/state', () => state);
vi.mock('#lib/queries/index.ts', () => ({
	useSessions: () => ({
		data: {
			sessions: [
				{ id: 's1', name: 'Fix the flaky test', working_dir: '/w', status: 'active', last_activity_at: '2026-11-01' }
			]
		}
	})
}));

import ErrorPage from './+error.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	state.page.status = 404;
	state.page.error = null;
});

const render = () => {
	comp = mount(ErrorPage, { target: document.body });
	flushSync();
};

describe('the not-found page', () => {
	it('centres a big status mark over the title and sentence', () => {
		render();
		const err = document.querySelector('.err') as HTMLElement;
		expect(err.querySelector('.mark')?.textContent?.replace(/\s+/g, '')).toBe('»_404');
		expect(err.querySelector('h1')?.textContent).toBe('Page not found');
		expect(err.textContent).toContain('Nothing lives at /does-not-exist.');
		expect(err.querySelector('a[href="/sessions/s1"]')).not.toBeNull();
	});

	it('marks other failures with their own status', () => {
		state.page.status = 500;
		render();
		expect(document.querySelector('.mark .code')?.textContent).toBe('500');
	});
});
