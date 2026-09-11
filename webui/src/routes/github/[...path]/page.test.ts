import { QueryClient } from '@tanstack/svelte-query';
import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The page reads its accounts through the shared connector store, so the stub
// has to answer like the real client: a 200 yields items, a non-2xx throws.
// That is what keeps "no connector" and "backend down" distinguishable here.
vi.mock('$lib/ghreview', () => ({
	ensureGhreviewToken: vi.fn().mockResolvedValue('tok'),
	listGhreviewAccounts: vi.fn(async () => {
		const res = await fetch(`${window.CCTUI_CONFIG?.ghreviewUrl}/v1/accounts`);
		if (!res.ok) throw new Error(`gh-review responded ${res.status}`);
		const body = (await res.json()) as { items?: { id: string; login: string }[] };
		return body.items ?? [];
	})
}));
vi.mock('$ghreview/Review.svelte', () => ({ default: function Review() {} }));

import Page from './+page.svelte';
import { resetGhreviewConnectors } from '$lib/ghreviewConnectors.svelte';

let component: ReturnType<typeof mount> | undefined;

const QUERY_CLIENT_CONTEXT_KEY = '$$_queryClient';
const context = new Map<string, unknown>([[QUERY_CLIENT_CONTEXT_KEY, new QueryClient()]]);

const tick = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
	delete window.CCTUI_CONFIG;
	// The store caches for the whole session; without this the second case's
	// answer would decide the third.
	resetGhreviewConnectors();
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

		expect(document.body.textContent).toContain('No GitHub connector yet');
	});

	it('reports an unreachable backend instead of claiming no connector exists', async () => {
		window.CCTUI_CONFIG = { ghreviewUrl: 'https://gh.example' };
		vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('boom', { status: 503 }));

		component = mount(Page, { target: document.body, context });
		for (let i = 0; i < 5; i++) await tick();

		expect(document.body.textContent).not.toContain('No GitHub connector yet');
		expect(document.body.textContent).toContain('Review center unavailable');
	});
});
