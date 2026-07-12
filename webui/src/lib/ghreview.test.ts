import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { get, post } = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }));
vi.mock('./api', () => ({ api: { get, post } }));

import { ensureGhreviewToken } from './ghreview';

beforeEach(() => {
	localStorage.clear();
	get.mockReset();
	post.mockReset();
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
