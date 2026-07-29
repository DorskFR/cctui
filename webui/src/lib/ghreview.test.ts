import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { get, post } = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }));
vi.mock('./api', () => ({ api: { get, post } }));

const { ghreviewUrl } = vi.hoisted(() => ({ ghreviewUrl: vi.fn() }));
vi.mock('./config', () => ({ ghreviewUrl }));

import {
	addGhreviewAccount,
	ensureGhreviewToken,
	listGhreviewAccounts,
	removeGhreviewAccount
} from './ghreview';

beforeEach(() => {
	localStorage.clear();
	sessionStorage.clear();
	get.mockReset();
	post.mockReset();
	ghreviewUrl.mockReset();
});

afterEach(() => {
	vi.restoreAllMocks();
});

describe('ensureGhreviewToken', () => {
	it('mints a scoped key for the current user and returns it', async () => {
		get.mockResolvedValue({ user_id: 'u1', scopes: ['sessions:read', 'github:read'] });
		post.mockResolvedValue({ id: 'k1', key: 'cctui_u_minted', scopes: ['sessions:read'] });

		const token = await ensureGhreviewToken();

		expect(token).toBe('cctui_u_minted');
		expect(get).toHaveBeenCalledWith('/me');
		const [path, body] = post.mock.calls[0];
		expect(path).toBe('/users/u1/keys');
		expect(body.scopes).toEqual(['read']);
	});

	it('caches in sessionStorage only, so the token dies with the tab', async () => {
		get.mockResolvedValue({ user_id: 'u1', scopes: [] });
		post.mockResolvedValue({ id: 'k1', key: 'cctui_u_minted', scopes: [] });

		await ensureGhreviewToken();

		expect(sessionStorage.getItem('cctui:ghreview-token')).toContain('cctui_u_minted');
		expect(localStorage.getItem('cctui:ghreview-token')).toBeNull();
	});

	it('reuses the cached token for the same user without re-minting', async () => {
		get.mockResolvedValue({ user_id: 'u1', scopes: [] });
		post.mockResolvedValue({ id: 'k1', key: 'cctui_u_first', scopes: [] });

		await ensureGhreviewToken();
		const second = await ensureGhreviewToken();

		expect(second).toBe('cctui_u_first');
		expect(post).toHaveBeenCalledTimes(1);
	});

	it('re-mints when the cached token belongs to a different user', async () => {
		get.mockResolvedValueOnce({ user_id: 'u1', scopes: [] });
		post.mockResolvedValueOnce({ id: 'k1', key: 'tok-u1', scopes: [] });
		await ensureGhreviewToken();

		get.mockResolvedValueOnce({ user_id: 'u2', scopes: [] });
		post.mockResolvedValueOnce({ id: 'k2', key: 'tok-u2', scopes: [] });
		const token = await ensureGhreviewToken();

		expect(token).toBe('tok-u2');
		expect(post).toHaveBeenCalledTimes(2);
	});

	it('rejects when the session has no user', async () => {
		get.mockResolvedValue({ user_id: null, scopes: [] });
		await expect(ensureGhreviewToken()).rejects.toThrow();
	});
});

function jsonResponse(status: number, body: unknown): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

function mockToken() {
	get.mockResolvedValue({ user_id: 'u1', scopes: [] });
	post.mockResolvedValue({ id: 'k1', key: 'tok', scopes: [] });
}

describe('listGhreviewAccounts', () => {
	it('returns an empty list without fetching when gh-review is unconfigured', async () => {
		ghreviewUrl.mockReturnValue(null);
		const fetchSpy = vi.spyOn(globalThis, 'fetch');
		expect(await listGhreviewAccounts()).toEqual([]);
		expect(fetchSpy).not.toHaveBeenCalled();
	});

	it('fetches the caller accounts with the minted bearer', async () => {
		ghreviewUrl.mockReturnValue('https://gh.example');
		mockToken();
		const fetchSpy = vi
			.spyOn(globalThis, 'fetch')
			.mockResolvedValue(jsonResponse(200, { items: [{ id: '7', login: 'octocat' }] }));

		const items = await listGhreviewAccounts();

		expect(items).toEqual([{ id: '7', login: 'octocat' }]);
		const [url, init] = fetchSpy.mock.calls[0];
		expect(url).toBe('https://gh.example/v1/accounts');
		expect((init?.headers as Record<string, string>).authorization).toBe('Bearer tok');
	});
});

describe('addGhreviewAccount', () => {
	it('posts the PAT and returns the created account', async () => {
		ghreviewUrl.mockReturnValue('https://gh.example');
		mockToken();
		const fetchSpy = vi
			.spyOn(globalThis, 'fetch')
			.mockResolvedValue(jsonResponse(201, { id: '7', login: 'octocat', created_at: null }));

		const account = await addGhreviewAccount('pat-xyz');

		expect(account.login).toBe('octocat');
		const [url, init] = fetchSpy.mock.calls[0];
		expect(url).toBe('https://gh.example/v1/accounts');
		expect(init?.method).toBe('POST');
		expect(JSON.parse(init?.body as string)).toEqual({ token: 'pat-xyz' });
		expect((init?.headers as Record<string, string>).authorization).toBe('Bearer tok');
	});

	it('includes the expected login when provided', async () => {
		ghreviewUrl.mockReturnValue('https://gh.example');
		mockToken();
		const fetchSpy = vi
			.spyOn(globalThis, 'fetch')
			.mockResolvedValue(jsonResponse(201, { id: '7', login: 'octocat', created_at: null }));

		await addGhreviewAccount('pat-xyz', 'octocat');

		expect(JSON.parse(fetchSpy.mock.calls[0][1]?.body as string)).toEqual({
			token: 'pat-xyz',
			login: 'octocat'
		});
	});

	it('surfaces a gh-review error message on failure (e.g. 409 conflict)', async () => {
		ghreviewUrl.mockReturnValue('https://gh.example');
		mockToken();
		vi.spyOn(globalThis, 'fetch').mockResolvedValue(
			jsonResponse(409, { error: { code: 'conflict', message: 'login owned by another user' } })
		);
		await expect(addGhreviewAccount('pat')).rejects.toThrow('login owned by another user');
	});
});

describe('removeGhreviewAccount', () => {
	it('deletes the account by id', async () => {
		ghreviewUrl.mockReturnValue('https://gh.example');
		mockToken();
		const fetchSpy = vi
			.spyOn(globalThis, 'fetch')
			.mockResolvedValue(new Response(null, { status: 204 }));

		await removeGhreviewAccount('7');

		const [url, init] = fetchSpy.mock.calls[0];
		expect(url).toBe('https://gh.example/v1/accounts/7');
		expect(init?.method).toBe('DELETE');
	});

	it('is a no-op without fetching when gh-review is unconfigured', async () => {
		ghreviewUrl.mockReturnValue(null);
		const fetchSpy = vi.spyOn(globalThis, 'fetch');
		await removeGhreviewAccount('7');
		expect(fetchSpy).not.toHaveBeenCalled();
	});

	it('tolerates a 404 (already gone)', async () => {
		ghreviewUrl.mockReturnValue('https://gh.example');
		mockToken();
		vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(null, { status: 404 }));
		await expect(removeGhreviewAccount('7')).resolves.toBeUndefined();
	});
});
