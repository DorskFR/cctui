import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const { ghreviewUrl } = vi.hoisted(() => ({ ghreviewUrl: vi.fn<() => string | null>() }));
vi.mock('$lib/config', () => ({ ghreviewUrl }));

const { listGhreviewAccounts } = vi.hoisted(() => ({ listGhreviewAccounts: vi.fn() }));
vi.mock('$lib/ghreview', () => ({ listGhreviewAccounts }));

vi.mock('$lib/queries', () => ({ useSessions: () => ({ data: { sessions: [] } }) }));
vi.mock('$app/state', () => ({ page: { url: new URL('https://app.test/sessions') } }));

import MainNav from './MainNav.svelte';
import {
	invalidateGhreviewConnectors,
	loadGhreviewConnectors,
	resetGhreviewConnectors
} from '$lib/ghreviewConnectors.svelte';

const account = (login: string) => ({ id: login, login, created_at: null });

function githubHrefs(target: HTMLElement): string[] {
	return [...target.querySelectorAll('a')]
		.map((a) => a.getAttribute('href') ?? '')
		.filter((h) => h === '/github');
}

let host: HTMLElement;
let component: Record<string, unknown>;

function render() {
	host = document.createElement('div');
	document.body.appendChild(host);
	component = mount(MainNav, { target: host, props: { placement: 'top' } });
	flushSync();
}

beforeEach(() => {
	document.body.innerHTML = '';
	resetGhreviewConnectors();
	ghreviewUrl.mockReturnValue('https://ghreview.example');
	listGhreviewAccounts.mockReset();
});

describe('MainNav github entry', () => {
	it('omits the entry while the connector list is still unknown', () => {
		render();
		expect(githubHrefs(host)).toEqual([]);
		unmount(component);
	});

	it('omits the entry when ghreview is deployed with zero connectors', async () => {
		listGhreviewAccounts.mockResolvedValue([]);
		await loadGhreviewConnectors();
		render();
		expect(githubHrefs(host)).toEqual([]);
		unmount(component);
	});

	it('shows the entry when a connector exists', async () => {
		listGhreviewAccounts.mockResolvedValue([account('octocat')]);
		await loadGhreviewConnectors();
		render();
		expect(githubHrefs(host)).toEqual(['/github']);
		unmount(component);
	});

	it('omits the entry when ghreviewUrl is unset even with connectors cached', async () => {
		listGhreviewAccounts.mockResolvedValue([account('octocat')]);
		await loadGhreviewConnectors();
		ghreviewUrl.mockReturnValue(null);
		render();
		expect(githubHrefs(host)).toEqual([]);
		unmount(component);
	});

	it('omits the entry when the lookup failed, without reloading', async () => {
		listGhreviewAccounts.mockRejectedValue(new Error('gh-review responded 502'));
		await loadGhreviewConnectors();
		render();
		expect(githubHrefs(host)).toEqual([]);
		unmount(component);
	});

	it('appears and disappears on invalidate without a remount', async () => {
		listGhreviewAccounts.mockResolvedValue([]);
		await loadGhreviewConnectors();
		render();
		expect(githubHrefs(host)).toEqual([]);

		listGhreviewAccounts.mockResolvedValue([account('octocat')]);
		await invalidateGhreviewConnectors();
		flushSync();
		expect(githubHrefs(host)).toEqual(['/github']);

		listGhreviewAccounts.mockResolvedValue([]);
		await invalidateGhreviewConnectors();
		flushSync();
		expect(githubHrefs(host)).toEqual([]);

		unmount(component);
	});
});
