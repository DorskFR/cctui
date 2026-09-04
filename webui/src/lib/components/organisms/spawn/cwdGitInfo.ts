import type { GitInfo } from '@bindings/GitInfo';

export const GIT_INFO_DEBOUNCE_MS = 300;

export type GitBadge = { text: string; worktree: boolean; sha?: string };

// `main`, `detached @abc1234`, or null when not a repo / nothing readable.
export function gitBadge(info: GitInfo | null | undefined): GitBadge | null {
	if (!info?.is_repo) return null;
	const worktree = info.is_worktree;
	if (info.branch) return { text: info.branch, worktree };
	if (info.detached_sha) {
		const sha = info.detached_sha.slice(0, 7);
		return { text: `detached @${sha}`, worktree, sha };
	}
	return null;
}

// Debounced (machine, path) lookup; only the latest request may deliver.
// Empty machine or path resolves to null immediately.
export function makeGitInfoWatcher(
	fetch: (machineId: string, path: string) => Promise<GitInfo>,
	onResult: (info: GitInfo | null) => void,
	delayMs = GIT_INFO_DEBOUNCE_MS
) {
	let timer: ReturnType<typeof setTimeout> | undefined;
	let seq = 0;
	function cancel() {
		seq++;
		if (timer !== undefined) clearTimeout(timer);
		timer = undefined;
	}
	function update(machineId: string, path: string) {
		cancel();
		if (!machineId || !path.trim()) {
			onResult(null);
			return;
		}
		const mine = seq;
		timer = setTimeout(() => {
			timer = undefined;
			fetch(machineId, path.trim()).then(
				(info) => {
					if (mine === seq) onResult(info);
				},
				() => {
					if (mine === seq) onResult(null);
				}
			);
		}, delayMs);
	}
	return { update, cancel };
}
