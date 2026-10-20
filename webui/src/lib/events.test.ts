import { describe, expect, it } from 'vitest';
import type { EventRecord } from '@bindings/EventRecord';
import {
	DEFAULT_FILTERS,
	actorKind,
	appendPage,
	filterQuery,
	formatRow,
	kindFamily,
	kindVerb,
	machineHistory,
	matchesFilters,
	mergeLive,
	nextCursor,
	sessionHref,
	sessionIdOf,
	severityDot,
	severityTone
} from './events';

const MACHINE = '0b6f2c1e-8a4d-4e2b-9f3a-5c7d1e2f3a4b';

function row(over: Partial<EventRecord> = {}): EventRecord {
	return {
		id: 1,
		occurred_at: '2026-10-07T10:00:00Z',
		kind: 'session.ended',
		severity: 'info',
		session_id: 's1',
		machine_id: MACHINE,
		user_id: null,
		actor: 'daemon',
		summary: 'wip ended (completed)',
		detail: { session_name: 'wip', machine_label: 'lab', end_reason: 'completed' },
		...over
	};
}

describe('kind rendering', () => {
	it('splits a kind into its family and a spaced verb', () => {
		expect(kindFamily('session.auto_resumed')).toBe('session');
		expect(kindVerb('session.auto_resumed')).toBe('auto resumed');
		expect(kindFamily('machine.offline')).toBe('machine');
		expect(kindFamily('system.reaper_ran')).toBe('system');
	});

	it('renders an unknown kind generically instead of dropping it', () => {
		expect(kindFamily('plugin.installed')).toBe('other');
		expect(kindVerb('plugin.installed')).toBe('installed');
		expect(kindVerb('bare')).toBe('bare');
		expect(formatRow(row({ kind: 'plugin.installed' })).verb).toBe('installed');
	});

	it('maps severity onto the dot and badge tones', () => {
		expect(severityDot('info')).toBe('active');
		expect(severityDot('warn')).toBe('stale');
		expect(severityDot('error')).toBe('dead');
		expect(severityDot('loud')).toBe('active');
		expect(severityTone('error')).toBe('danger');
		expect(severityTone('warn')).toBe('warn');
		expect(severityTone('info')).toBe('info');
	});

	it('classifies actors by their prefix', () => {
		expect(actorKind('user:3f9a')).toBe('user');
		expect(actorKind('agent:s1')).toBe('agent');
		expect(actorKind('daemon')).toBe('daemon');
		expect(actorKind('reaper')).toBe('reaper');
		expect(actorKind('system')).toBe('system');
		expect(actorKind('martian')).toBe('system');
	});

	it('keeps a deleted session readable: name from detail, no link, id from detail', () => {
		const gone = row({ session_id: null, detail: { session_name: 'wip', session_id: 's1' } });
		const view = formatRow(gone);
		expect(view.sessionName).toBe('wip');
		expect(view.sessionHref).toBeNull();
		expect(sessionIdOf(gone)).toBe('s1');
		expect(sessionHref(row())).toBe('/sessions/s1');
		expect(sessionHref(row({ session_id: 'a/b' }))).toBe('/sessions/a%2Fb');
	});

	it('tolerates a detail that is not an object', () => {
		const view = formatRow(row({ detail: 'x' as unknown as EventRecord['detail'] }));
		expect(view.sessionName).toBeNull();
		expect(view.machineLabel).toBeNull();
	});
});

describe('filter logic', () => {
	it('turns filters into the query the API takes and nothing for the defaults', () => {
		expect(filterQuery(DEFAULT_FILTERS)).toEqual({});
		expect(filterQuery({ family: 'machine', severity: 'warn', machineId: MACHINE })).toEqual({
			kind: 'machine.',
			severity: 'warn',
			machine_id: MACHINE
		});
		expect(filterQuery({ ...DEFAULT_FILTERS, family: 'other' })).toEqual({});
	});

	it('matches live rows the same way the server list would', () => {
		expect(matchesFilters(row(), DEFAULT_FILTERS)).toBe(true);
		expect(matchesFilters(row(), { ...DEFAULT_FILTERS, family: 'machine' })).toBe(false);
		expect(matchesFilters(row({ kind: 'machine.offline' }), { ...DEFAULT_FILTERS, family: 'machine' })).toBe(true);
		expect(matchesFilters(row(), { ...DEFAULT_FILTERS, severity: 'warn' })).toBe(false);
		expect(matchesFilters(row({ severity: 'warn' }), { ...DEFAULT_FILTERS, severity: 'warn' })).toBe(true);
		expect(matchesFilters(row(), { ...DEFAULT_FILTERS, machineId: 'other' })).toBe(false);
		expect(matchesFilters(row(), { ...DEFAULT_FILTERS, machineId: MACHINE })).toBe(true);
	});

	it('prepends a live row once, in id order, and never duplicates a refetched one', () => {
		const shown = [row({ id: 5 }), row({ id: 3 })];
		expect(mergeLive(shown, row({ id: 7 })).map((r) => r.id)).toEqual([7, 5, 3]);
		expect(mergeLive(shown, row({ id: 5 }))).toBe(shown);
		expect(mergeLive(shown, row({ id: 4 })).map((r) => r.id)).toEqual([5, 4, 3]);
		expect(mergeLive([], row({ id: 1 })).map((r) => r.id)).toEqual([1]);
	});

	it('appends a page under the shown rows, skipping ids already delivered live', () => {
		const shown = [row({ id: 9 }), row({ id: 8 })];
		const page = [row({ id: 8 }), row({ id: 7 })];
		expect(appendPage(shown, page).map((r) => r.id)).toEqual([9, 8, 7]);
		expect(nextCursor(appendPage(shown, page))).toBe(7);
		expect(nextCursor([])).toBeNull();
	});

	it('keeps only reachability and enrolment kinds for the machine history', () => {
		const rows = [
			row({ id: 1, kind: 'machine.online' }),
			row({ id: 2, kind: 'machine.daemon_disconnected' }),
			row({ id: 3, kind: 'session.ended' }),
			row({ id: 4, kind: 'machine.updated' })
		];
		expect(machineHistory(rows).map((r) => r.id)).toEqual([1, 2, 4]);
	});
});
