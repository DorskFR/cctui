import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const { ghreviewUrl } = vi.hoisted(() => ({ ghreviewUrl: vi.fn<() => string | null>() }));
vi.mock('$lib/config', () => ({ ghreviewUrl }));

const { listGhreviewAccounts, ensureGhreviewToken } = vi.hoisted(() => ({
	listGhreviewAccounts: vi.fn(),
	ensureGhreviewToken: vi.fn()
}));
vi.mock('$lib/ghreview', () => ({ listGhreviewAccounts, ensureGhreviewToken }));
vi.mock('$ghreview/Review.svelte', () => ({ default: () => undefined }));

import Page from './[...path]/+page.svelte';
import { resetGhreviewConnectors } from '$lib/ghreviewConnectors.svelte';

const settle = async () => {
	for (let i = 0; i < 20; i++) {
		await new Promise((r) => setTimeout(r, 0));
		flushSync();
	}
};

beforeEach(() => {
	document.body.innerHTML = '';
	resetGhreviewConnectors();
	ghreviewUrl.mockReturnValue('https://ghreview.example');
	listGhreviewAccounts.mockReset();
	ensureGhreviewToken.mockResolvedValue('tok');
});

describe('/github route with no connector', () => {
	it('renders the unlock screen pointing at Accounts', async () => {
		listGhreviewAccounts.mockResolvedValue([]);
		const host = document.createElement('div');
		document.body.appendChild(host);
		const component = mount(Page, { target: host });
		await settle();

		expect(host.textContent).toContain('GitHub');
		expect([...host.querySelectorAll('a')].map((a) => a.getAttribute('href'))).toContain(
			'/accounts'
		);
		unmount(component);
	});

	it('renders the unavailable panel, not the unlock screen, when the lookup fails', async () => {
		listGhreviewAccounts.mockRejectedValue(new Error('gh-review responded 502'));
		const host = document.createElement('div');
		document.body.appendChild(host);
		const component = mount(Page, { target: host });
		await settle();

		expect([...host.querySelectorAll('a')].map((a) => a.getAttribute('href'))).not.toContain(
			'/accounts'
		);
		unmount(component);
	});
});
