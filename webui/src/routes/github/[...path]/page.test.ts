import { QueryClient } from '@tanstack/svelte-query';
import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('$lib/ghreview', () => ({ ensureGhreviewToken: vi.fn().mockResolvedValue('tok') }));
vi.mock('$ghreview/Review.svelte', () => ({ default: function Review() {} }));

import Page from './+page.svelte';

let component: ReturnType<typeof mount> | undefined;

const QUERY_CLIENT_CONTEXT_KEY = '$$_queryClient';
const context = new Map<string, unknown>([[QUERY_CLIENT_CONTEXT_KEY, new QueryClient()]]);

const tick = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
	delete window.CCTUI_CONFIG;
});

afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
	delete window.CCTUI_CONFIG;
	vi.restoreAllMocks();
});

describe('/github page (graceful degradation)', () => {
	it('renders the not-configured panel when ghreviewUrl is unset', () => {
		component = mount(Page, { target: document.body, context });
		expect(document.body.textContent).toContain('Review center not configured');
	});

	it('renders the linked-account empty state when configured but no accounts exist', async () => {
		window.CCTUI_CONFIG = { ghreviewUrl: 'https://gh.example' };
		vi.spyOn(globalThis, 'fetch').mockResolvedValue(
			new Response(JSON.stringify({ items: [] }), {
				status: 200,
				headers: { 'content-type': 'application/json' }
			})
		);

		component = mount(Page, { target: document.body, context });
		for (let i = 0; i < 5; i++) await tick();

		expect(document.body.textContent).toContain('No review account linked yet');
	});
});
