import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { get, post } = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }));
vi.mock('./api', () => ({ api: { get, post } }));

const { ghreviewUrl } = vi.hoisted(() => ({ ghreviewUrl: vi.fn() }));
vi.mock('./config', () => ({ ghreviewUrl }));

import {
	deprovisionGhreviewAccount,
	ensureGhreviewToken,
	provisionGhreviewAccount
} from './ghreview';

beforeEach(() => {
	localStorage.clear();
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
		expect(body.scopes).toEqual(['sessions:read', 'github:read']);
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

describe('provisionGhreviewAccount', () => {
	it('is a no-op returning null when gh-review is unconfigured', async () => {
		ghreviewUrl.mockReturnValue(null);
		const fetchSpy = vi.spyOn(globalThis, 'fetch');
		expect(await provisionGhreviewAccount('c1', 'pat')).toBeNull();
		expect(fetchSpy).not.toHaveBeenCalled();
	});

	it('posts the PAT, returns the derived login, and remembers the mapping', async () => {
		ghreviewUrl.mockReturnValue('https://gh.example');
		mockToken();
		const fetchSpy = vi
			.spyOn(globalThis, 'fetch')
			.mockResolvedValue(jsonResponse(201, { id: '7', login: 'octocat' }));

		const login = await provisionGhreviewAccount('c1', 'pat-xyz');

		expect(login).toBe('octocat');
		const [url, init] = fetchSpy.mock.calls[0];
		expect(url).toBe('https://gh.example/v1/accounts');
		expect(init?.method).toBe('POST');
		expect(JSON.parse(init?.body as string)).toEqual({ token: 'pat-xyz' });
		expect((init?.headers as Record<string, string>).authorization).toBe('Bearer tok');
		expect(localStorage.getItem('cctui:ghreview-connector-logins')).toContain('octocat');
	});

	it('surfaces a gh-review error message on failure (e.g. 409 conflict)', async () => {
		ghreviewUrl.mockReturnValue('https://gh.example');
		mockToken();
		vi.spyOn(globalThis, 'fetch').mockResolvedValue(
			jsonResponse(409, { error: { code: 'conflict', message: 'login owned by another user' } })
		);
		await expect(provisionGhreviewAccount('c1', 'pat')).rejects.toThrow(
			'login owned by another user'
		);
	});
});

describe('deprovisionGhreviewAccount', () => {
	it('deletes the account matching the cached login', async () => {
		ghreviewUrl.mockReturnValue('https://gh.example');
		mockToken();
		const fetchSpy = vi
			.spyOn(globalThis, 'fetch')
			.mockResolvedValueOnce(jsonResponse(201, { id: '7', login: 'octocat' }));
		await provisionGhreviewAccount('c1', 'pat');

		fetchSpy.mockResolvedValueOnce(
			jsonResponse(200, { items: [{ id: '7', login: 'octocat' }] })
		);
		fetchSpy.mockResolvedValueOnce(new Response(null, { status: 204 }));

		await deprovisionGhreviewAccount('c1');

		const del = fetchSpy.mock.calls[2];
		expect(del[0]).toBe('https://gh.example/v1/accounts/7');
		expect(del[1]?.method).toBe('DELETE');
		expect(localStorage.getItem('cctui:ghreview-connector-logins')).not.toContain('octocat');
	});

	it('is a no-op when no login was cached for the connector', async () => {
		ghreviewUrl.mockReturnValue('https://gh.example');
		mockToken();
		const fetchSpy = vi.spyOn(globalThis, 'fetch');
		await deprovisionGhreviewAccount('unknown');
		expect(fetchSpy).not.toHaveBeenCalled();
	});
});
