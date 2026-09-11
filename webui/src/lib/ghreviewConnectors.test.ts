import { beforeEach, describe, expect, it, vi } from 'vitest';

const { ghreviewUrl } = vi.hoisted(() => ({ ghreviewUrl: vi.fn<() => string | null>() }));
vi.mock('./config', () => ({ ghreviewUrl }));

const { listGhreviewAccounts } = vi.hoisted(() => ({ listGhreviewAccounts: vi.fn() }));
vi.mock('./ghreview', () => ({ listGhreviewAccounts }));

import {
	connectorStatus,
	ghreviewAccounts,
	hasGithubConnector,
	invalidateGhreviewConnectors,
	loadGhreviewConnectors,
	resetGhreviewConnectors
} from './ghreviewConnectors.svelte';

const account = (login: string) => ({ id: login, login, created_at: null });

beforeEach(() => {
	resetGhreviewConnectors();
	ghreviewUrl.mockReturnValue('https://ghreview.example');
	listGhreviewAccounts.mockReset();
});

describe('ghreview connector store', () => {
	it('reads false before anything is loaded', () => {
		expect(hasGithubConnector()).toBe(false);
		expect(connectorStatus()).toBe('unknown');
	});

	it('does not call the backend when ghreviewUrl is unset', async () => {
		ghreviewUrl.mockReturnValue(null);
		await loadGhreviewConnectors();
		expect(listGhreviewAccounts).not.toHaveBeenCalled();
		expect(connectorStatus()).toBe('unset');
		expect(hasGithubConnector()).toBe(false);
	});

	it('reports none for an empty connector list', async () => {
		listGhreviewAccounts.mockResolvedValue([]);
		await loadGhreviewConnectors();
		expect(connectorStatus()).toBe('none');
		expect(hasGithubConnector()).toBe(false);
	});

	it('reports some and exposes the accounts', async () => {
		listGhreviewAccounts.mockResolvedValue([account('octocat')]);
		await loadGhreviewConnectors();
		expect(hasGithubConnector()).toBe(true);
		expect(ghreviewAccounts().map((a) => a.login)).toEqual(['octocat']);
	});

	it('caches a resolved answer for the session', async () => {
		listGhreviewAccounts.mockResolvedValue([account('octocat')]);
		await loadGhreviewConnectors();
		await loadGhreviewConnectors();
		expect(listGhreviewAccounts).toHaveBeenCalledTimes(1);
	});

	it('coalesces concurrent loads into one request', async () => {
		listGhreviewAccounts.mockResolvedValue([]);
		await Promise.all([loadGhreviewConnectors(), loadGhreviewConnectors()]);
		expect(listGhreviewAccounts).toHaveBeenCalledTimes(1);
	});

	it('does not report a failed lookup as "no connector"', async () => {
		listGhreviewAccounts.mockRejectedValue(new Error('gh-review responded 502'));
		await loadGhreviewConnectors();
		expect(connectorStatus()).toBe('error');
		expect(connectorStatus()).not.toBe('none');
		expect(hasGithubConnector()).toBe(false);
	});

	it('retries after a failure instead of caching it', async () => {
		listGhreviewAccounts.mockRejectedValueOnce(new Error('down'));
		await loadGhreviewConnectors();
		listGhreviewAccounts.mockResolvedValue([account('octocat')]);
		await loadGhreviewConnectors();
		expect(hasGithubConnector()).toBe(true);
	});

	it('picks up a first connector on invalidate', async () => {
		listGhreviewAccounts.mockResolvedValue([]);
		await loadGhreviewConnectors();
		expect(hasGithubConnector()).toBe(false);

		listGhreviewAccounts.mockResolvedValue([account('octocat')]);
		await invalidateGhreviewConnectors();
		expect(hasGithubConnector()).toBe(true);
	});

	it('drops the last connector on invalidate', async () => {
		listGhreviewAccounts.mockResolvedValue([account('octocat')]);
		await loadGhreviewConnectors();
		expect(hasGithubConnector()).toBe(true);

		listGhreviewAccounts.mockResolvedValue([]);
		await invalidateGhreviewConnectors();
		expect(hasGithubConnector()).toBe(false);
		expect(ghreviewAccounts()).toEqual([]);
	});
});
