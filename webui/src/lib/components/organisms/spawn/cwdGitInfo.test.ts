import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { GitInfo } from '@bindings/GitInfo';
import { gitBadge, makeGitInfoWatcher } from './cwdGitInfo';

const repo = (over: Partial<GitInfo>): GitInfo => ({
	is_repo: true,
	is_worktree: false,
	...over
});

describe('gitBadge', () => {
	it('shows the branch, the short detached sha, or nothing', () => {
		expect(gitBadge(repo({ branch: 'main' }))).toEqual({ text: 'main', worktree: false });
		expect(gitBadge(repo({ detached_sha: '0123456789abcdef0123456789abcdef01234567' }))).toEqual({
			text: 'detached @0123456',
			worktree: false,
			sha: '0123456'
		});
		expect(gitBadge(repo({ branch: 'wt', is_worktree: true }))?.worktree).toBe(true);
		expect(gitBadge({ is_repo: false, is_worktree: false })).toBeNull();
		expect(gitBadge(null)).toBeNull();
	});
});

describe('makeGitInfoWatcher', () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it('debounces and delivers only the latest lookup', async () => {
		const fetch = vi.fn(async (_m: string, path: string) => repo({ branch: path }));
		const results: (GitInfo | null)[] = [];
		const w = makeGitInfoWatcher(fetch, (i) => results.push(i), 300);
		w.update('m1', '/a');
		w.update('m1', '/b');
		await vi.advanceTimersByTimeAsync(299);
		expect(fetch).not.toHaveBeenCalled();
		await vi.advanceTimersByTimeAsync(1);
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(fetch).toHaveBeenCalledWith('m1', '/b');
		expect(results).toEqual([repo({ branch: '/b' })]);
	});

	it('clears immediately without a machine or path and swallows errors', async () => {
		const fetch = vi.fn(async () => {
			throw new Error('offline');
		});
		const results: (GitInfo | null)[] = [];
		const w = makeGitInfoWatcher(fetch, (i) => results.push(i), 300);
		w.update('', '/a');
		w.update('m1', '  ');
		expect(fetch).not.toHaveBeenCalled();
		expect(results).toEqual([null, null]);
		w.update('m1', '/a');
		await vi.advanceTimersByTimeAsync(300);
		expect(results).toEqual([null, null, null]);
	});

	it('drops a reply that lands after a newer update or cancel', async () => {
		let resolve!: (i: GitInfo) => void;
		const fetch = vi.fn(() => new Promise<GitInfo>((r) => (resolve = r)));
		const results: (GitInfo | null)[] = [];
		const w = makeGitInfoWatcher(fetch, (i) => results.push(i), 300);
		w.update('m1', '/a');
		await vi.advanceTimersByTimeAsync(300);
		w.cancel();
		resolve(repo({ branch: 'late' }));
		await vi.advanceTimersByTimeAsync(0);
		expect(results).toEqual([]);
	});
});
