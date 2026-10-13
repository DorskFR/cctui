import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from './api';

const post = vi.fn();
vi.mock('./api', async (orig) => ({
	...(await orig<typeof import('./api')>()),
	api: { post: (...a: unknown[]) => post(...a) }
}));

const { joinAfterSpawn, isSessionMissing, JOIN_BUDGET_MS } = await import('./cctuiverseJoin');

const missing = () => new ApiError(404, 'no such session');
let clock = 0;
const opts = (probe: () => Promise<unknown>) => ({
	probe,
	now: () => clock,
	sleep: async (ms: number) => {
		clock += ms;
	}
});

beforeEach(() => {
	clock = 0;
	post.mockReset();
});

describe('joinAfterSpawn', () => {
	it('waits for the session row before joining', async () => {
		const probe = vi.fn().mockResolvedValueOnce(null).mockRejectedValueOnce(new Error('net')).mockResolvedValue({ id: 's1' });
		post.mockResolvedValue({ link: { id: 'l1' } });
		await expect(joinAfterSpawn('inv', 's1', 'me', opts(probe))).resolves.toEqual({ id: 'l1' });
		expect(probe).toHaveBeenCalledTimes(3);
		expect(post).toHaveBeenCalledTimes(1);
		expect(post).toHaveBeenCalledWith('/cctuiverse/join', { invite: 'inv', session_id: 's1', label: 'me' });
	});

	it('retries a join that still races registration', async () => {
		post.mockRejectedValueOnce(missing()).mockRejectedValueOnce(missing()).mockResolvedValue({ link: { id: 'l1' } });
		await expect(joinAfterSpawn('inv', 's1', 'me', opts(async () => ({})))).resolves.toEqual({ id: 'l1' });
		expect(post).toHaveBeenCalledTimes(3);
	});

	it('does not retry other refusals', async () => {
		post.mockRejectedValue(new ApiError(410, 'invite already used'));
		await expect(joinAfterSpawn('inv', 's1', 'me', opts(async () => ({})))).rejects.toThrow('invite already used');
		expect(post).toHaveBeenCalledTimes(1);
	});

	it('does not retry a 404 other than the missing session', async () => {
		post.mockRejectedValue(new ApiError(404, 'no such invite'));
		await expect(joinAfterSpawn('inv', 's1', 'me', opts(async () => ({})))).rejects.toThrow('no such invite');
		expect(post).toHaveBeenCalledTimes(1);
	});

	it('gives up after the budget with the real error', async () => {
		post.mockRejectedValue(missing());
		await expect(joinAfterSpawn('inv', 's1', 'me', opts(async () => ({})))).rejects.toThrow('no such session');
		expect(clock).toBeGreaterThanOrEqual(JOIN_BUDGET_MS);
		expect(clock).toBeLessThan(JOIN_BUDGET_MS + 5_000);
	});

	it('still tries the join once when the row never shows', async () => {
		post.mockRejectedValue(missing());
		const probe = vi.fn().mockResolvedValue(null);
		await expect(joinAfterSpawn('inv', 's1', 'me', opts(probe))).rejects.toThrow('no such session');
		expect(post).toHaveBeenCalledTimes(1);
	});
});

describe('isSessionMissing', () => {
	it('matches only the registration race', () => {
		expect(isSessionMissing(missing())).toBe(true);
		expect(isSessionMissing(new ApiError(403, 'no such session'))).toBe(false);
		expect(isSessionMissing(new Error('no such session'))).toBe(false);
	});
});
