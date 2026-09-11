import { ghreviewUrl } from './config';
import { listGhreviewPulls } from './ghreview';
import { ghreviewAccounts, hasGithubConnector } from './ghreviewConnectors.svelte';
import { parseGithubRemote } from './gitRemote';

export interface BranchPull {
	number: number;
	url: string;
	state: string;
}

export const PULL_CACHE_TTL_MS = 5 * 60 * 1000;

interface Entry {
	at: number;
	value: BranchPull | null;
}

const cache = new Map<string, Entry>();
const inflight = new Map<string, Promise<BranchPull | null>>();

export function pullCacheKey(owner: string, repo: string, branch: string): string {
	return `${owner.toLowerCase()}/${repo.toLowerCase()}#${branch}`;
}

export function cachedBranchPull(key: string, now = Date.now()): BranchPull | null | undefined {
	const hit = cache.get(key);
	if (!hit) return undefined;
	if (now - hit.at >= PULL_CACHE_TTL_MS) {
		cache.delete(key);
		return undefined;
	}
	return hit.value;
}

export function resetBranchPullCache(): void {
	cache.clear();
	inflight.clear();
}

/** Swallowing every failure to null is the contract, not an oversight. */
export function resolveBranchPull(
	remoteUrl: string | null | undefined,
	branch: string | null | undefined,
	now = Date.now()
): Promise<BranchPull | null> {
	if (!branch) return Promise.resolve(null);
	if (ghreviewUrl() === null || !hasGithubConnector()) return Promise.resolve(null);
	const remote = parseGithubRemote(remoteUrl);
	if (!remote) return Promise.resolve(null);

	const key = pullCacheKey(remote.owner, remote.repo, branch);
	const hit = cachedBranchPull(key, now);
	if (hit !== undefined) return Promise.resolve(hit);
	const pending = inflight.get(key);
	if (pending) return pending;

	const account = ghreviewAccounts()[0]?.login;
	const run = listGhreviewPulls(remote.owner, remote.repo, account)
		.then((pulls) => {
			const match = pulls.find((p) => p.head?.ref === branch && p.state === 'open');
			const value: BranchPull | null =
				match?.number !== undefined && match.html_url
					? { number: match.number, url: match.html_url, state: match.draft ? 'draft' : 'open' }
					: null;
			cache.set(key, { at: now, value });
			return value;
		})
		.catch(() => null)
		.finally(() => {
			inflight.delete(key);
		});
	inflight.set(key, run);
	return run;
}
