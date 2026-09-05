import { describe, expect, it } from 'vitest';
import type { LimitResetStatus } from '$lib/queries';
import { limitResetHint, limitResetLabel } from './limit-reset';

const status = (extra: Partial<LimitResetStatus> = {}): LimitResetStatus => ({
	kind: 'claude',
	available: false,
	...extra
});

describe('limitResetLabel', () => {
	it('names the codex credit when there is one', () => {
		expect(limitResetLabel(status({ kind: 'codex', title: 'Full reset (Weekly + 5 hr)' }))).toContain(
			'Full reset (Weekly + 5 hr)'
		);
		expect(limitResetLabel(status())).not.toContain('{');
	});
});

describe('limitResetHint', () => {
	it('is empty when the reset is claimable', () => {
		expect(limitResetHint(status({ available: true }))).toBe('');
	});
	it('carries the upstream reason and the next window', () => {
		const hint = limitResetHint(
			status({ ineligible_reason: 'not_at_wall', next_available_at: new Date(Date.now() + 3600_000).toISOString() })
		);
		expect(hint).toContain('not_at_wall');
		expect(hint.split('\n')).toHaveLength(2);
	});
	it('falls back to a generic line', () => {
		expect(limitResetHint(status())).not.toBe('');
	});
});
