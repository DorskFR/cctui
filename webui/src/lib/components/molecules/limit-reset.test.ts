// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import type { LimitResetStatus } from '$lib/queries';
import { limitResetClears, limitResetHint, limitResetLabel } from './limit-reset';

const status = (extra: Partial<LimitResetStatus> = {}): LimitResetStatus => ({
	kind: 'claude',
	available: false,
	title: null,
	credit_id: null,
	ineligible_reason: null,
	next_available_at: null,
	weekly_resets_at: null,
	...extra
});

const grant = (extra: Partial<LimitResetStatus> = {}): LimitResetStatus =>
	status({
		title: 'Get extra wiggle room to explore Opus 5.5',
		credit_id: 'opus_55_explore',
		next_available_at: '2026-10-23T00:00:00Z',
		resets_left: 2,
		requires_limit: false,
		clears: ['five_hour', 'seven_day'],
		...extra
	});

describe('limitResetLabel', () => {
	it('names the codex credit when there is one', () => {
		expect(limitResetLabel(status({ kind: 'codex', title: 'Full reset (Weekly + 5 hr)' }))).toContain(
			'Full reset (Weekly + 5 hr)'
		);
		expect(limitResetLabel(status())).not.toContain('{');
	});
	it('names the claude grant label when there is one', () => {
		expect(limitResetLabel(grant())).toContain('explore Opus 5.5');
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
	it('shows a cedar grant expiry instead of a next window', () => {
		const hint = limitResetHint(
			grant({ available: false, ineligible_reason: 'tier', next_available_at: '2026-10-23T00:00:00Z' })
		);
		expect(hint).toContain('tier');
		expect(hint).toMatch(/2026|23/);
		expect(hint).not.toContain('{');
	});
	it('says a grant may only be spent at a limit', () => {
		expect(limitResetHint(grant({ requires_limit: true }))).not.toBe(
			limitResetHint(grant({ requires_limit: false }))
		);
		expect(limitResetHint(grant({ requires_limit: false, next_available_at: null }))).not.toContain('{');
	});
});

describe('limitResetClears', () => {
	it('names the windows a claim refills, once each', () => {
		const text = limitResetClears(grant({ clears: ['five_hour', 'seven_day', 'seven_day_opus'] }));
		expect(text).not.toContain('{');
		expect(text.split('+')).toHaveLength(2);
	});
	it('is empty when upstream named no window', () => {
		expect(limitResetClears(status())).toBe('');
		expect(limitResetClears(grant({ clears: [] }))).toBe('');
	});
});
