import { beforeAll, describe, expect, it } from 'vitest';
import type { DomainMeta } from '@bindings/DomainMeta';
import type { EndReasonInfo } from '@bindings/EndReasonInfo';
import { setDomainMeta } from './domainMeta.svelte';
import { endBadgeText, endReasonTone, sessionEnd, sessionEndTitle } from './sessionEnd';

// The tone/muted rules are the server's; this mirrors what `/meta/domain`
// serves so the mapping can be exercised without a network.
const END_REASONS: EndReasonInfo[] = [
	{ reason: 'completed', tone: 'ok', muted: false, failed_start: false },
	{ reason: 'killed', tone: 'neutral', muted: false, failed_start: false },
	{ reason: 'crashed', tone: 'danger', muted: false, failed_start: false },
	{ reason: 'daemon_lost', tone: 'warn', muted: false, failed_start: false },
	{ reason: 'machine_offline', tone: 'warn', muted: false, failed_start: false },
	{ reason: 'reaped_inactive', tone: 'neutral', muted: true, failed_start: false },
	{ reason: 'resume_failed', tone: 'danger', muted: false, failed_start: true },
	{ reason: 'spawn_failed', tone: 'danger', muted: false, failed_start: true },
	{ reason: 'other', tone: 'neutral', muted: false, failed_start: false }
];

beforeAll(() => {
	setDomainMeta({
		providers: [],
		usage_probes: [],
		end_reasons: END_REASONS,
		permission_modes: ['ask', 'auto', 'yolo', 'whip']
	} satisfies DomainMeta);
});

describe('sessionEnd', () => {
	it('is null for a live session', () => {
		expect(sessionEnd({ end_reason: null, end_detail: null, ended_at: null })).toBeNull();
		expect(sessionEnd({})).toBeNull();
	});

	it('takes each reason’s colour from the server table', () => {
		for (const row of END_REASONS) expect(endReasonTone(row.reason)).toBe(row.tone);
	});

	it('puts the first line of a failed start into the badge, truncated', () => {
		const failed = sessionEnd({
			end_reason: 'spawn_failed',
			end_detail: 'unknown model gpt-nope; available: gpt-5-codex\nsecond line',
			ended_at: '2026-09-04T10:00:00Z'
		});
		expect(failed?.badge).toBe('failed: unknown model gpt-nope; available: gpt-5-codex');
		expect(endBadgeText('spawn_failed', 'failed', `${'a'.repeat(60)}\nb`)).toBe(
			`failed: ${'a'.repeat(47)}…`
		);
		expect(endBadgeText('crashed', 'crashed', 'boom')).toBe('crashed');
		expect(endBadgeText('resume_failed', 'resume failed', null)).toBe('resume failed');
		expect(endBadgeText('resume_failed', 'resume failed', 'auth')).toBe('resume failed: auth');
	});

	it('marks reaped sessions muted and carries the detail into the tooltip', () => {
		const reaped = sessionEnd({ end_reason: 'reaped_inactive', ended_at: '2026-09-04T10:00:00Z' });
		expect(reaped?.muted).toBe(true);
		expect(reaped?.detail).toBeNull();
		const crashed = sessionEnd({
			end_reason: 'crashed',
			end_detail: '  claude -p exited (exit status: 1); last stderr:\nboom  ',
			ended_at: '2026-09-04T10:00:00Z'
		});
		expect(crashed?.muted).toBe(false);
		expect(crashed?.detail).toBe('claude -p exited (exit status: 1); last stderr:\nboom');
		if (!crashed) throw new Error('expected an end');
		const title = sessionEndTitle(crashed);
		expect(title).toContain('2026');
		expect(title).toContain('exit status: 1');
	});
});
