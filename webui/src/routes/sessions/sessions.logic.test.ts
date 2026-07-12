import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import {
	fmtWhen,
	formatAgo,
	isSection,
	matchesUnreadFilter,
	parseSections,
	sessionDebugRows,
	toolActivity,
	TOOL_ASLEEP_AFTER_MS,
	type Section
} from './sessions.logic';

function session(over: Partial<SessionListItem>): SessionListItem {
	return { id: 'sess-abc', unread_count: 0, ...over } as SessionListItem;
}

describe('unread section (CCT-580)', () => {
	it('recognises "unread" as a section', () => {
		expect(isSection('unread')).toBe(true);
	});

	it('round-trips through parseSections', () => {
		expect(parseSections('live,unread')).toEqual(new Set<Section>(['live', 'unread']));
	});

	it('is off by default (never in the fallback set)', () => {
		expect(parseSections(null).has('unread')).toBe(false);
		expect(parseSections('').has('unread')).toBe(false);
	});

	describe('matchesUnreadFilter', () => {
		const on = new Set<Section>(['live', 'unread']);
		const off = new Set<Section>(['live']);

		it('passes every row when the filter is off', () => {
			expect(matchesUnreadFilter(session({ unread_count: 0 }), off)).toBe(true);
			expect(matchesUnreadFilter(session({ unread_count: 3 }), off)).toBe(true);
		});

		it('keeps only rows with unread messages when on', () => {
			expect(matchesUnreadFilter(session({ unread_count: 0 }), on)).toBe(false);
			expect(matchesUnreadFilter(session({ unread_count: 1 }), on)).toBe(true);
		});

		it('treats a missing count as zero unread', () => {
			expect(matchesUnreadFilter(session({ unread_count: undefined }), on)).toBe(false);
		});
	});
});

describe('tool activity — asleep vs. grinding (CCT-594)', () => {
	const NOW = 1_000_000_000_000;
	const working = (over: Partial<SessionListItem>) =>
		session({ bucket: 'working', status: 'active', ...over });

	it('is hidden with no tool activity and no headline', () => {
		expect(toolActivity(working({}), NOW).show).toBe(false);
	});

	it('shows the headline even before any tool call', () => {
		const a = toolActivity(working({ activity_detail: 'compiling…' }), NOW);
		expect(a.show).toBe(true);
		expect(a.detail).toBe('compiling…');
		expect(a.ageMs).toBeNull();
		expect(a.asleep).toBe(false);
	});

	it('reads as grinding when the last tool call is fresh', () => {
		const a = toolActivity(
			working({ last_tool_at: new Date(NOW - 10_000).toISOString(), tool_use_count: 42 }),
			NOW
		);
		expect(a.show).toBe(true);
		expect(a.count).toBe(42);
		expect(a.ageMs).toBe(10_000);
		expect(a.asleep).toBe(false);
	});

	it('reads as asleep once tool calls stop past the threshold', () => {
		const a = toolActivity(
			working({ last_tool_at: new Date(NOW - TOOL_ASLEEP_AFTER_MS - 1_000).toISOString() }),
			NOW
		);
		expect(a.asleep).toBe(true);
		expect(a.show).toBe(true);
	});

	it('is never asleep for a non-working bucket', () => {
		const a = toolActivity(
			session({
				bucket: 'done',
				status: 'active',
				last_tool_at: new Date(NOW - TOOL_ASLEEP_AFTER_MS - 1_000).toISOString()
			}),
			NOW
		);
		expect(a.asleep).toBe(false);
		expect(a.show).toBe(false);
	});
});

describe('debug tooltip rows (CCT-555)', () => {
	const NOW = 1_000_000_000_000;

	it('renders nulls as "—", never "null"', () => {
		const rows = sessionDebugRows(session({ liveness: 'dead' }), NOW);
		const values = Object.fromEntries(rows.map((r) => [r.label, r.value]));
		expect(values.account).toBe('—');
		expect(values.created).toBe('—');
		expect(values.machine).toBe('—');
		expect(values.keepalive).toBe('—');
		expect(JSON.stringify(rows)).not.toContain('null');
	});

	it('shows account, machine (+ non-persistent kind) and credential state', () => {
		const rows = sessionDebugRows(
			session({
				account_name: 'work',
				machine_name: 'runner-1',
				machine_kind: 'dispatch',
				has_token_credentials: true
			}),
			NOW
		);
		const values = Object.fromEntries(rows.map((r) => [r.label, r.value]));
		expect(values.account).toBe('work');
		expect(values.machine).toBe('runner-1 (dispatch)');
		expect(values.creds).toBe('live token binding');
	});

	it('omits the kind suffix for persistent machines', () => {
		const rows = sessionDebugRows(
			session({ machine_name: 'laptop', machine_kind: 'persistent' }),
			NOW
		);
		expect(rows.find((r) => r.label === 'machine')?.value).toBe('laptop');
	});

	it('flags an account with no live token binding', () => {
		const rows = sessionDebugRows(
			session({ account_name: 'work', has_token_credentials: false }),
			NOW
		);
		expect(rows.find((r) => r.label === 'creds')?.value).toBe(
			'account only (token revoked/absent)'
		);
	});

	it('reports hibernated as the status word', () => {
		const rows = sessionDebugRows(session({ hibernated: true, liveness: 'dead' }), NOW);
		expect(rows.find((r) => r.label === 'status')?.value).toBe('hibernated');
	});
});

describe('fmtWhen', () => {
	const NOW = 1_000_000_000_000;
	it('returns "—" for missing or unparseable timestamps', () => {
		expect(fmtWhen(null, NOW)).toBe('—');
		expect(fmtWhen(undefined, NOW)).toBe('—');
		expect(fmtWhen('not-a-date', NOW)).toBe('—');
	});
	it('pairs a relative age with the raw ISO', () => {
		const iso = new Date(NOW - 90_000).toISOString();
		expect(fmtWhen(iso, NOW)).toBe(`1m ago · ${iso}`);
	});
});

describe('formatAgo', () => {
	it('formats seconds, minutes, hours', () => {
		expect(formatAgo(5_000)).toBe('5s');
		expect(formatAgo(90_000)).toBe('1m');
		expect(formatAgo(3 * 3600_000)).toBe('3h');
		expect(formatAgo(-100)).toBe('0s');
	});
});
