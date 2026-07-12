import { QueryClient } from '@tanstack/svelte-query';
import { mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import Page from './+page.svelte';

let component: ReturnType<typeof mount> | undefined;

const QUERY_CLIENT_CONTEXT_KEY = '$$_queryClient';
const context = new Map<string, unknown>([[QUERY_CLIENT_CONTEXT_KEY, new QueryClient()]]);

afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
	delete window.CCTUI_CONFIG;
});

describe('/github page (graceful degradation)', () => {
	it('renders the not-configured panel when ghreviewUrl is unset', () => {
		component = mount(Page, { target: document.body, context });
		expect(document.body.textContent).toContain('Review center not configured');
	});
});
