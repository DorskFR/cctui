import { describe, expect, it, vi } from 'vitest';
import { isHttpError } from '@sveltejs/kit';

const { session } = vi.hoisted(() => ({ session: vi.fn() }));
vi.mock('#lib/queries/endpoints.ts', () => ({ endpoints: { session } }));

import { ApiError } from '#lib/api.ts';
import { load } from './+page.ts';

const run = () => load({ params: { session: 'abc' } } as Parameters<typeof load>[0]);

describe('the session deep link', () => {
	it('loads quietly when the session exists', async () => {
		session.mockResolvedValueOnce({ id: 'abc' });
		await expect(run()).resolves.toBeUndefined();
	});

	it('raises a 404 for a deleted session, keeping the URL', async () => {
		session.mockRejectedValueOnce(new ApiError(404, 'not found'));
		let err: unknown;
		try {
			await run();
		} catch (e) {
			err = e;
		}
		expect(isHttpError(err, 404)).toBe(true);
		expect(isHttpError(err) && err.body.message).toBe('session_missing');
	});

	it('leaves other failures to the sessions page', async () => {
		session.mockRejectedValueOnce(new ApiError(503, 'down'));
		await expect(run()).resolves.toBeUndefined();
	});
});
