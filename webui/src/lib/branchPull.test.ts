import { beforeEach, describe, expect, it, vi } from 'vitest';

const { ghreviewUrl } = vi.hoisted(() => ({ ghreviewUrl: vi.fn<() => string | null>() }));
vi.mock('./config', () => ({ ghreviewUrl }));

const { listGhreviewPulls } = vi.hoisted(() => ({ listGhreviewPulls: vi.fn() }));
vi.mock('./ghreview', () => ({ listGhreviewPulls }));

const { hasGithubConnector, ghreviewAccounts } = vi.hoisted(() => ({
	hasGithubConnector: vi.fn<() => boolean>(),
	ghreviewAccounts: vi.fn<() => { id: string; login: string }[]>()
}));
vi.mock('./ghreviewConnectors.svelte', () => ({ hasGithubConnector, ghreviewAccounts }));

import {
	PULL_CACHE_TTL_MS,
	cachedBranchPull,
	pullCacheKey,
	resetBranchPullCache,
	resolveBranchPull
} from './branchPull';

const REMOTE = 'git@github.com:DorskFR/cctui.git';
const openPull = {
	number: 123,
	html_url: 'https://github.com/DorskFR/cctui/pull/123',
	state: 'open',
	draft: false,
	head: { ref: 'feature' }
};

beforeEach(() => {
	resetBranchPullCache();
	ghreviewUrl.mockReturnValue('https://ghreview.example');
	hasGithubConnector.mockReturnValue(true);
	ghreviewAccounts.mockReturnValue([{ id: 'a', login: 'DorskFR' }]);
	listGhreviewPulls.mockReset();
	listGhreviewPulls.mockResolvedValue([openPull]);
});

describe('resolveBranchPull no-ops', () => {
	it('makes no request without a branch', async () => {
		expect(await resolveBranchPull(REMOTE, null)).toBeNull();
		expect(listGhreviewPulls).not.toHaveBeenCalled();
	});

	it('makes no request without an origin remote', async () => {
		expect(await resolveBranchPull(null, 'feature')).toBeNull();
		expect(await resolveBranchPull('', 'feature')).toBeNull();
		expect(listGhreviewPulls).not.toHaveBeenCalled();
	});

	it('makes no request for a non-GitHub remote', async () => {
		expect(await resolveBranchPull('git@gitlab.com:DorskFR/cctui.git', 'feature')).toBeNull();
		expect(listGhreviewPulls).not.toHaveBeenCalled();
	});

	it('makes no request when ghreview is not configured', async () => {
		ghreviewUrl.mockReturnValue(null);
		expect(await resolveBranchPull(REMOTE, 'feature')).toBeNull();
		expect(listGhreviewPulls).not.toHaveBeenCalled();
	});

	it('makes no request when there is no GitHub connector', async () => {
		hasGithubConnector.mockReturnValue(false);
		expect(await resolveBranchPull(REMOTE, 'feature')).toBeNull();
		expect(listGhreviewPulls).not.toHaveBeenCalled();
	});

	it('swallows an unreachable or rate-limited backend', async () => {
		listGhreviewPulls.mockRejectedValue(new Error('gh-review responded 429'));
		await expect(resolveBranchPull(REMOTE, 'feature')).resolves.toBeNull();
	});

	it('returns null when no open pull matches the branch', async () => {
		listGhreviewPulls.mockResolvedValue([{ ...openPull, head: { ref: 'other' } }]);
		expect(await resolveBranchPull(REMOTE, 'feature')).toBeNull();
	});

	it('ignores a closed pull on the same branch', async () => {
		listGhreviewPulls.mockResolvedValue([{ ...openPull, state: 'closed' }]);
		expect(await resolveBranchPull(REMOTE, 'feature')).toBeNull();
	});
});

describe('resolveBranchPull matching', () => {
	it('returns the open pull for the branch', async () => {
		expect(await resolveBranchPull(REMOTE, 'feature')).toEqual({
			number: 123,
			url: 'https://github.com/DorskFR/cctui/pull/123',
			state: 'open'
		});
	});

	it('reports a draft pull as draft', async () => {
		listGhreviewPulls.mockResolvedValue([{ ...openPull, draft: true }]);
		expect((await resolveBranchPull(REMOTE, 'feature'))?.state).toBe('draft');
	});

	it('passes the first connector login as the account', async () => {
		await resolveBranchPull(REMOTE, 'feature');
		expect(listGhreviewPulls).toHaveBeenCalledWith('DorskFR', 'cctui', 'DorskFR');
	});
});

describe('branch pull cache', () => {
	it('keys case-insensitively on the repo and exactly on the branch', () => {
		expect(pullCacheKey('DorskFR', 'CCTUI', 'feature')).toBe(pullCacheKey('dorskfr', 'cctui', 'feature'));
		expect(pullCacheKey('o', 'r', 'a')).not.toBe(pullCacheKey('o', 'r', 'A'));
	});

	it('serves a repeat lookup from the cache', async () => {
		await resolveBranchPull(REMOTE, 'feature', 1000);
		await resolveBranchPull(REMOTE, 'feature', 1000);
		expect(listGhreviewPulls).toHaveBeenCalledTimes(1);
	});

	it('caches a negative answer too', async () => {
		listGhreviewPulls.mockResolvedValue([]);
		expect(await resolveBranchPull(REMOTE, 'feature', 1000)).toBeNull();
		expect(await resolveBranchPull(REMOTE, 'feature', 1000)).toBeNull();
		expect(listGhreviewPulls).toHaveBeenCalledTimes(1);
	});

	it('does not cache across branches or repos', async () => {
		await resolveBranchPull(REMOTE, 'feature', 1000);
		await resolveBranchPull(REMOTE, 'other', 1000);
		await resolveBranchPull('git@github.com:DorskFR/other.git', 'feature', 1000);
		expect(listGhreviewPulls).toHaveBeenCalledTimes(3);
	});

	it('re-fetches once the entry expires', async () => {
		await resolveBranchPull(REMOTE, 'feature', 1000);
		await resolveBranchPull(REMOTE, 'feature', 1000 + PULL_CACHE_TTL_MS);
		expect(listGhreviewPulls).toHaveBeenCalledTimes(2);
	});

	it('coalesces concurrent lookups of the same key', async () => {
		await Promise.all([
			resolveBranchPull(REMOTE, 'feature', 1000),
			resolveBranchPull(REMOTE, 'feature', 1000)
		]);
		expect(listGhreviewPulls).toHaveBeenCalledTimes(1);
	});

	it('reports a miss as undefined, distinct from a cached null', async () => {
		const key = pullCacheKey('DorskFR', 'cctui', 'feature');
		expect(cachedBranchPull(key, 1000)).toBeUndefined();
		listGhreviewPulls.mockResolvedValue([]);
		await resolveBranchPull(REMOTE, 'feature', 1000);
		expect(cachedBranchPull(key, 1000)).toBeNull();
	});

	it('is cleared by reset', async () => {
		await resolveBranchPull(REMOTE, 'feature', 1000);
		resetBranchPullCache();
		await resolveBranchPull(REMOTE, 'feature', 1000);
		expect(listGhreviewPulls).toHaveBeenCalledTimes(2);
	});
});
