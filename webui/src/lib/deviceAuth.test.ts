import { describe, expect, it } from 'vitest';
import {
	codeFromSearch,
	isCompleteUserCode,
	isDecidable,
	normalizeUserCode,
	requesterFacts,
	statusTone
} from './deviceAuth';

describe('normalizeUserCode', () => {
	it('uppercases, drops the separator the user typed and re-groups', () => {
		expect(normalizeUserCode('bcdf2345')).toBe('BCDF-2345');
		expect(normalizeUserCode('bcdf 2345')).toBe('BCDF-2345');
		expect(normalizeUserCode('BCDF-2345')).toBe('BCDF-2345');
	});

	it('leaves a partial code ungrouped so typing is not fought', () => {
		expect(normalizeUserCode('bcd')).toBe('BCD');
		expect(normalizeUserCode('')).toBe('');
	});

	it('never grows past one code', () => {
		expect(normalizeUserCode('BCDF-2345-6789')).toBe('BCDF-2345');
	});
});

describe('isCompleteUserCode', () => {
	it('is true only for a full eight symbols', () => {
		expect(isCompleteUserCode('bcdf2345')).toBe(true);
		expect(isCompleteUserCode('BCDF-234')).toBe(false);
		expect(isCompleteUserCode('')).toBe(false);
	});
});

describe('codeFromSearch', () => {
	it('reads and normalizes the code a verification link carries', () => {
		expect(codeFromSearch('?code=bcdf2345')).toBe('BCDF-2345');
		expect(codeFromSearch(new URLSearchParams({ code: 'BCDF-2345' }))).toBe('BCDF-2345');
	});

	it('is empty when there is nothing to read', () => {
		expect(codeFromSearch('')).toBe('');
		expect(codeFromSearch('?other=1')).toBe('');
	});
});

describe('status', () => {
	it('lets the user decide only a pending request', () => {
		expect(isDecidable('pending')).toBe(true);
		for (const s of ['approved', 'denied', 'expired'] as const) {
			expect(isDecidable(s)).toBe(false);
		}
	});

	it('tones a dead request as a danger and an approved one as success', () => {
		expect(statusTone('pending')).toBe('info');
		expect(statusTone('approved')).toBe('success');
		expect(statusTone('denied')).toBe('danger');
		expect(statusTone('expired')).toBe('danger');
	});
});

describe('requesterFacts', () => {
	const base = {
		client_ip: '203.0.113.7',
		user_agent: 'cctui/0.23',
		created_at: '2026-10-01T12:00:00Z'
	};
	const nowMs = Date.parse('2026-10-01T12:00:30Z');

	it('shows what the requester did not get to choose', () => {
		expect(requesterFacts(base, nowMs)).toEqual([
			{ label: 'from', value: '203.0.113.7' },
			{ label: 'client', value: 'cctui/0.23' },
			{ label: 'started', value: '30s ago' }
		]);
	});

	it('omits what the deployment did not record rather than showing blanks', () => {
		expect(requesterFacts({ ...base, client_ip: null, user_agent: null }, nowMs)).toEqual([
			{ label: 'started', value: '30s ago' }
		]);
	});

	it('spells a longer wait in minutes, so a stale request is obvious', () => {
		const later = Date.parse('2026-10-01T12:07:05Z');
		expect(requesterFacts(base, later).at(-1)).toEqual({ label: 'started', value: '7m 5s ago' });
	});

	it('drops an unparseable timestamp instead of rendering NaN', () => {
		expect(requesterFacts({ ...base, created_at: 'not a date' }, nowMs)).toEqual([
			{ label: 'from', value: '203.0.113.7' },
			{ label: 'client', value: 'cctui/0.23' }
		]);
	});
});
