import { mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import Page from './+page.svelte';

let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
	delete window.CCTUI_CONFIG;
});

describe('/review page (graceful degradation)', () => {
	it('renders the not-configured panel when ghreviewUrl is unset', () => {
		component = mount(Page, { target: document.body });
		expect(document.body.textContent).toContain('Review center not configured');
	});
});
