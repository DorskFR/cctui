import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { GitInfo } from '@bindings/GitInfo';
import type { MachineLiveness } from '@bindings/MachineLiveness';
import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
import { fsLiveness, gitBadge, makeGitInfoWatcher } from './cwdGitInfo';

const repo = (over: Partial<GitInfo>): GitInfo => ({
	is_repo: true,
	is_worktree: false,
	...over
});

const machine = (
	machine_id: string,
	liveness: MachineLiveness,
	connected = liveness !== 'offline'
): MachineResourcesRow => ({
	machine_id,
	name: machine_id,
	display_name: null,
	hue: null,
	liveness,
	connected,
	last_seen_at: '2026-10-22T00:00:00Z',
	resources: null,
	updated_at: null
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
		w.update('m1', '/a', 'online');
		w.update('m1', '/b', 'online');
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
		w.update('', '/a', 'online');
		w.update('m1', '  ', 'online');
		expect(fetch).not.toHaveBeenCalled();
		expect(results).toEqual([null, null]);
		w.update('m1', '/a', 'stale');
		await vi.advanceTimersByTimeAsync(300);
		expect(results).toEqual([null, null, null]);
	});

	it('never asks a machine known offline, and asks again once it is back', async () => {
		const fetch = vi.fn(async () => repo({ branch: 'main' }));
		const results: (GitInfo | null)[] = [];
		const w = makeGitInfoWatcher(fetch, (i) => results.push(i), 300);
		w.update('m1', '/a', 'offline');
		await vi.advanceTimersByTimeAsync(300);
		expect(fetch).not.toHaveBeenCalled();
		expect(results).toEqual([null]);
		w.update('m1', '/a', 'online');
		await vi.advanceTimersByTimeAsync(300);
		expect(fetch).toHaveBeenCalledWith('m1', '/a');
		expect(results).toEqual([null, repo({ branch: 'main' })]);
	});

	it('waits for the machine list before asking, then skips offline and asks online', async () => {
		const fetch = vi.fn(async () => repo({ branch: 'main' }));
		const results: (GitInfo | null)[] = [];
		const w = makeGitInfoWatcher(fetch, (i) => results.push(i), 300);
		const liveness = (list: MachineResourcesRow[]) =>
			list.find((mc) => mc.machine_id === 'm1')?.liveness;
		w.update('m1', '/a', liveness([]));
		await vi.advanceTimersByTimeAsync(300);
		expect(fetch).not.toHaveBeenCalled();
		w.update('m1', '/a', liveness([machine('m1', 'offline')]));
		await vi.advanceTimersByTimeAsync(300);
		expect(fetch).not.toHaveBeenCalled();
		w.update('m1', '/a', liveness([machine('m1', 'online')]));
		await vi.advanceTimersByTimeAsync(300);
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(results).toEqual([null, null, repo({ branch: 'main' })]);
	});

	it('never asks gitinfo of an online machine whose daemon socket is down', async () => {
		const fetch = vi.fn(async () => repo({ branch: 'main' }));
		const results: (GitInfo | null)[] = [];
		const w = makeGitInfoWatcher(fetch, (i) => results.push(i), 300);
		const rows = (connected: boolean) => [machine('m1', 'online', connected)];
		const of = (list: MachineResourcesRow[]) =>
			fsLiveness(list.find((mc) => mc.machine_id === 'm1'));
		expect(of([])).toBeUndefined();
		expect(of(rows(false))).toBe('offline');
		w.update('m1', '/a', of(rows(false)));
		await vi.advanceTimersByTimeAsync(300);
		expect(fetch).not.toHaveBeenCalled();
		expect(results).toEqual([null]);
		w.update('m1', '/a', of(rows(true)));
		await vi.advanceTimersByTimeAsync(300);
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(results).toEqual([null, repo({ branch: 'main' })]);
	});

	it('drops a reply that lands after a newer update or cancel', async () => {
		let resolve!: (i: GitInfo) => void;
		const fetch = vi.fn(() => new Promise<GitInfo>((r) => (resolve = r)));
		const results: (GitInfo | null)[] = [];
		const w = makeGitInfoWatcher(fetch, (i) => results.push(i), 300);
		w.update('m1', '/a', 'online');
		await vi.advanceTimersByTimeAsync(300);
		w.cancel();
		resolve(repo({ branch: 'late' }));
		await vi.advanceTimersByTimeAsync(0);
		expect(results).toEqual([]);
	});
});
