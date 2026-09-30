import { describe, expect, it } from 'vitest';
import type { GitInfo } from '@bindings/GitInfo';
import { gitBadge } from '$lib/components/organisms/spawn/cwdGitInfo';
import { parityFixture } from './fixtures';

type Case = {
	info: Partial<GitInfo> | null;
	out: { text: string; worktree: boolean; sha?: string } | null;
};

const fx = parityFixture<Case[]>('gitBadge');

describe('gitBadge parity fixtures', () => {
	it('matches the shared badge rule', () => {
		for (const c of fx) {
			expect(gitBadge(c.info as GitInfo | null), JSON.stringify(c)).toEqual(c.out);
		}
	});
});
