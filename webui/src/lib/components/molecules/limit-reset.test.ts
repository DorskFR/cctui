// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import type { LimitResetEntry, LimitResetStatus } from '$lib/queries';
import {
	limitResetClears,
	limitResetHint,
	limitResetLabel,
	limitResetTooltip,
	nextResetToExpire,
	resetReason,
	resetRestores,
	resetTitle
} from './limit-reset';

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

const entry = (extra: Partial<LimitResetEntry> = {}): LimitResetEntry => ({
	kind: 'claude',
	id: 'opus_55_explore',
	title: 'Get extra wiggle room to explore Opus 5.5',
	restores: ['five_hour', 'seven_day'],
	expires_at: '2026-10-23T00:00:00Z',
	resets_left: 2,
	requires_limit: false,
	usable: true,
	unusable_reason: null,
	...extra
});

describe('resetTitle', () => {
	it('uses the upstream title, and names the at-wall program when there is none', () => {
		expect(resetTitle(entry())).toContain('explore Opus 5.5');
		expect(resetTitle(entry({ id: 'juniper_tide', title: null }))).not.toContain('{');
		expect(resetTitle(entry({ id: 'juniper_tide', title: null }))).not.toBe(
			resetTitle(entry({ id: 'cr_1', title: null }))
		);
	});
});

describe('resetRestores', () => {
	it('names each window once and stays empty when upstream named none', () => {
		expect(resetRestores(entry({ restores: ['five_hour', 'seven_day', 'seven_day_opus'] }))).toBe(
			resetRestores(entry({ restores: ['five_hour', 'seven_day'] }))
		);
		expect(resetRestores(entry({ restores: [] }))).toBe('');
	});
});

describe('resetReason', () => {
	it('is empty for a usable entry and names the upstream reason otherwise', () => {
		expect(resetReason(entry())).toBe('');
		const paused = resetReason(entry({ usable: false, unusable_reason: 'paused' }));
		expect(paused).toContain('paused');
		expect(paused).not.toContain('{');
	});
	it('adds the at-a-limit rule and falls back to a generic line', () => {
		const strict = resetReason(entry({ usable: false, unusable_reason: null, requires_limit: true }));
		expect(strict.split('\n')).toHaveLength(1);
		expect(strict).not.toContain('{');
		expect(resetReason(entry({ usable: false, unusable_reason: null, requires_limit: false }))).not.toBe('');
	});
});

describe('nextResetToExpire', () => {
	it('picks the soonest expiry even when a later one sorts first', () => {
		const soon = entry({ id: 'soon', expires_at: '2026-10-23T00:00:00Z' });
		const late = entry({ id: 'late', expires_at: '2026-10-30T00:00:00Z' });
		expect(nextResetToExpire([late, soon])?.id).toBe('soon');
	});
	it('prefers a dated entry, else falls back to the first', () => {
		const undated = entry({ id: 'juniper_tide', expires_at: null });
		const dated = entry({ id: 'dated' });
		expect(nextResetToExpire([undated, dated])?.id).toBe('dated');
		expect(nextResetToExpire([undated])?.id).toBe('juniper_tide');
		expect(nextResetToExpire([])).toBe(null);
	});
});

describe('limitResetTooltip', () => {
	it('names the next reset and its date when a claim is available', () => {
		const text = limitResetTooltip([entry()], status({ available: true, title: 'x' }));
		expect(text).toContain('explore Opus 5.5');
		expect(text).toMatch(/2026|23/);
		expect(text).not.toContain('{');
		expect(text.split('\n')).toHaveLength(1);
	});

	it('counts the resets beyond the one it names', () => {
		const two = [entry({ id: 'a' }), entry({ id: 'b', expires_at: '2026-10-30T00:00:00Z' })];
		const text = limitResetTooltip(two, status({ available: true }));
		expect(text).toContain('1');
		expect(text.split('\n')).toHaveLength(2);
		expect(limitResetTooltip([entry()], status({ available: true }))).not.toContain('+');
	});

	it('still names the next reset when nothing is claimable, and keeps the reason', () => {
		const text = limitResetTooltip(
			[entry({ usable: false, unusable_reason: 'paused' })],
			status({ available: false, ineligible_reason: 'not_at_wall' })
		);
		expect(text).toContain('explore Opus 5.5');
		expect(text).toContain('not_at_wall');
		expect(text).not.toContain('{');
	});

	it('falls back to the button label with no entries', () => {
		expect(limitResetTooltip([], status({ available: true }))).toBe(
			limitResetLabel(status({ available: true }))
		);
		expect(limitResetTooltip([], null)).toBe('');
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
