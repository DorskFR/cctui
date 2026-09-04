import { describe, expect, it } from 'vitest';
import { endReasonTone, sessionEnd, sessionEndTitle } from './sessionEnd';

describe('sessionEnd', () => {
	it('is null for a live session', () => {
		expect(sessionEnd({ end_reason: null, end_detail: null, ended_at: null })).toBeNull();
		expect(sessionEnd({})).toBeNull();
	});

	it('maps each reason to its colour', () => {
		expect(endReasonTone('completed')).toBe('ok');
		expect(endReasonTone('killed')).toBe('neutral');
		expect(endReasonTone('crashed')).toBe('danger');
		expect(endReasonTone('resume_failed')).toBe('danger');
		expect(endReasonTone('daemon_lost')).toBe('warn');
		expect(endReasonTone('machine_offline')).toBe('warn');
		expect(endReasonTone('reaped_inactive')).toBe('neutral');
		expect(endReasonTone('other')).toBe('neutral');
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
