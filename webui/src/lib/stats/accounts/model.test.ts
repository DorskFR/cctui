import { describe, expect, it } from 'vitest';
import type { AccountLedgerRow } from '@bindings/AccountLedgerRow';
import type { AccountLedgerTotal } from '@bindings/AccountLedgerTotal';
import type { AccountUsageEntry } from '@bindings/AccountUsageEntry';
import type { UsageWindowView } from '@bindings/UsageWindowView';
import {
	credentialLabels,
	currentRows,
	filterLedger,
	hoursText,
	ledgerFrom,
	ledgerOptions,
	ledgerRowTone,
	lossTone,
	lostText,
	oneLiners,
	patchUsageEntries,
	resetClock,
	sortLedger,
	totalGroups,
	totalText,
	usageOf
} from './model';

const row = (over: Partial<AccountLedgerRow> = {}): AccountLedgerRow => ({
	account_id: 'c1',
	account_name: 'personal',
	account_emoji: null,
	provider: 'anthropic',
	window_key: 'weekly_all',
	window_label: 'Weekly',
	resets_at: '2026-10-22T11:00:00Z',
	final_utilization: 94,
	unused_pct: 6,
	unused_capacity_hours: 10.08,
	amount_usd: null,
	sampling_gap: false,
	idle: false,
	...over
});

const win = (key: string, utilization: number, resets_at = '2026-10-22T15:00:00Z'): UsageWindowView => ({
	key,
	kind: key,
	label: key,
	utilization,
	resets_at
});

const entry = (
	id: string,
	name: string,
	windows: UsageWindowView[],
	provider = 'anthropic'
): AccountUsageEntry => ({
	account: `acct-${name}`,
	account_name: name,
	account_emoji: null,
	header_pin: true,
	account_id: id,
	provider,
	usage: null,
	windows,
	age_secs: 0,
	limit_reset: null,
	limit_resets: []
});

const utcClock = (iso: string) => resetClock(iso, Date.parse('2026-10-22T20:00:00Z'), 'en-GB', 'UTC');

describe('ledgerFrom', () => {
	it('starts seven days back, on the hour, so views share one cache entry', () => {
		const a = ledgerFrom(Date.parse('2026-10-22T11:05:00Z'));
		const b = ledgerFrom(Date.parse('2026-10-22T11:55:00Z'));
		expect(a).toBe('2026-10-15T11:00:00.000Z');
		expect(b).toBe(a);
	});
});

describe('usage cache', () => {
	const old = [entry('a1', 'one', [win('session', 10)]), entry('a2', 'two', [win('session', 20)])];
	const pushed = (id: string, utilization: number) => ({
		account_id: id,
		provider: 'anthropic',
		usage: null,
		windows: [win('session', utilization)],
		age_secs: 0,
		limit_reset: null
	});

	it('patches the usage half of the pushed credential and keeps its identity', () => {
		const next = patchUsageEntries(old, 'a1', pushed('a1', 72));
		expect(next?.[0].windows[0].utilization).toBe(72);
		expect(next?.[0].account_name).toBe('one');
		expect(next?.[0].header_pin).toBe(true);
		expect(next?.[1]).toBe(old[1]);
	});

	it('returns the same array for a credential it does not hold', () => {
		expect(patchUsageEntries(old, 'other', pushed('other', 5))).toBe(old);
		expect(patchUsageEntries(undefined, 'a1', pushed('a1', 5))).toBeUndefined();
	});

	it('selects one credential off the batch', () => {
		expect(usageOf(old, 'a2')?.account_name).toBe('two');
		expect(usageOf(old, 'nope')).toBeNull();
		expect(usageOf(undefined, 'a1')).toBeNull();
	});
});

describe('credentialLabels', () => {
	it('adds the provider only when an account holds several credentials', () => {
		const labels = credentialLabels([
			{ id: 'c1', name: 'personal', provider: 'anthropic' },
			{ id: 'c2', name: 'personal', provider: 'openai' },
			{ id: 'c3', name: 'work', provider: 'anthropic' }
		]);
		expect(labels.get('c1')).toBe('personal · anthropic');
		expect(labels.get('c2')).toBe('personal · openai');
		expect(labels.get('c3')).toBe('work');
	});
});

describe('currentRows', () => {
	it('builds one row per credential with a 5h or weekly window', () => {
		const rows = currentRows([
			entry('c1', 'personal', [win('session', 42), win('weekly_all', 61)]),
			entry('c2', 'work', [win('opus_weekly', 10)]),
			entry('c3', 'spare', [])
		]);
		expect(rows.map((r) => r.id)).toEqual(['c1']);
		expect(rows[0].fiveHour).toEqual({ pct: 42, resetsAt: '2026-10-22T15:00:00Z' });
		expect(rows[0].weekly?.pct).toBe(61);
	});

	it('leaves a missing window null instead of zero', () => {
		const [r] = currentRows([entry('c1', 'personal', [win('session', 5)])]);
		expect(r.weekly).toBeNull();
	});
});

describe('lostText / tones', () => {
	it('reads idle resets as idle, never as 100% lost', () => {
		const idle = row({ idle: true, final_utilization: 0, unused_pct: 100 });
		expect(lostText(idle)).toBe('idle');
		expect(lossTone(idle)).toBe('unknown');
		expect(ledgerRowTone(idle)).toBeUndefined();
	});

	it('marks a sampling gap as an upper bound', () => {
		expect(lostText(row({ unused_pct: 6.4 }))).toBe('6%');
		expect(lostText(row({ unused_pct: 6.4, sampling_gap: true }))).toBe('≤6%');
	});

	it('tones the loss by the named loss thresholds', () => {
		expect(lossTone(row({ unused_pct: 6 }))).toBe('ok');
		expect(lossTone(row({ unused_pct: 20 }))).toBe('warn');
		expect(ledgerRowTone(row({ unused_pct: 55 }))).toBe('danger');
	});

	it('formats capacity hours, dropping the decimal past ten', () => {
		expect(hoursText(0.3)).toBe('≈ 0.3 h');
		expect(hoursText(10.08)).toBe('≈ 10 h');
		expect(hoursText(null)).toBeNull();
	});
});

describe('filterLedger / sortLedger / ledgerOptions', () => {
	const rows = [
		row({ account_id: 'c1', window_key: 'session', window_label: '5h', unused_pct: 30, resets_at: '2026-10-22T10:00:00Z' }),
		row({ account_id: 'c1', unused_pct: 6, resets_at: '2026-10-22T11:00:00Z' }),
		row({ account_id: 'c2', account_name: 'work', idle: true, unused_pct: 100, resets_at: '2026-10-21T11:00:00Z' })
	];

	it('filters by credential and window', () => {
		expect(filterLedger(rows, { account: 'c1', window: null })).toHaveLength(2);
		expect(filterLedger(rows, { account: 'c1', window: 'session' })).toHaveLength(1);
		expect(filterLedger(rows, { account: null, window: null })).toHaveLength(3);
	});

	it('sorts idle resets last in both directions on usage columns', () => {
		const lostDesc = sortLedger(rows, 'lost', 'desc').map((r) => r.unused_pct);
		const lostAsc = sortLedger(rows, 'lost', 'asc').map((r) => r.unused_pct);
		expect(lostDesc).toEqual([30, 6, 100]);
		expect(lostAsc).toEqual([6, 30, 100]);
	});

	it('sorts by reset time', () => {
		const asc = sortLedger(rows, 'resets_at', 'asc').map((r) => r.resets_at);
		expect(asc).toEqual(['2026-10-21T11:00:00Z', '2026-10-22T10:00:00Z', '2026-10-22T11:00:00Z']);
	});

	it('offers each credential and window once', () => {
		const o = ledgerOptions(rows);
		expect(o.accounts).toEqual([
			{ value: 'c1', label: 'personal' },
			{ value: 'c2', label: 'work' }
		]);
		expect(o.windows.map((w) => w.value).sort()).toEqual(['session', 'weekly_all']);
	});
});

describe('oneLiners', () => {
	it('summarises each credential by its latest loss', () => {
		const lines = oneLiners(
			[
				row({ resets_at: '2026-10-22T11:00:00Z', unused_pct: 6 }),
				row({ window_key: 'session', resets_at: '2026-10-22T09:00:00Z', unused_pct: 40 })
			],
			utcClock
		);
		expect(lines.map((l) => l.text)).toEqual(['personal: lost 6% weekly at 11:00']);
	});

	it('skips idle resets for the latest loss and calls a fully idle credential idle', () => {
		const lines = oneLiners(
			[
				row({ resets_at: '2026-10-22T12:00:00Z', idle: true, unused_pct: 100 }),
				row({ resets_at: '2026-10-22T11:00:00Z', unused_pct: 6, sampling_gap: true }),
				row({ account_id: 'c2', account_name: 'work', idle: true, unused_pct: 100 })
			],
			utcClock
		);
		expect(lines.map((l) => l.text)).toEqual([
			'personal: lost ≤6% weekly at 11:00',
			'work: idle'
		]);
		expect(lines[1].tone).toBe('unknown');
	});

	it('keeps one line per credential, never one for the pool', () => {
		const lines = oneLiners(
			[row({ account_id: 'c1' }), row({ account_id: 'c2', provider: 'openai', window_key: 'session', unused_pct: 12 })],
			utcClock
		);
		expect(lines).toHaveLength(2);
		expect(lines.map((l) => l.text)).toEqual([
			'personal · anthropic: lost 6% weekly at 11:00',
			'personal · openai: lost 12% 5h at 11:00'
		]);
	});

	it('names the day for a reset before today', () => {
		expect(utcClock('2026-10-20T11:00:00Z')).toBe('Tue 11:00');
	});
});

describe('totals', () => {
	const total = (over: Partial<AccountLedgerTotal> = {}): AccountLedgerTotal => ({
		account_id: 'c1',
		account_name: 'personal',
		window_key: 'session',
		closes: 5,
		unused_pct_mean: 31.6,
		unused_capacity_hours: 4.2,
		idle_closes: 2,
		...over
	});

	it('reports resets, idle count and the mean of the used ones', () => {
		expect(totalText(total())).toBe('5h: 5 resets, 2 idle, 32% lost on average when used · ≈ 4.2 h');
	});

	it('calls a window idle when every reset was', () => {
		expect(totalText(total({ idle_closes: 5, unused_pct_mean: null, unused_capacity_hours: null }))).toBe(
			'5h: idle (5 resets)'
		);
	});

	it('groups windows under their credential', () => {
		const groups = totalGroups(
			[total(), total({ window_key: 'weekly_all' }), total({ account_id: 'c2', account_name: 'work' })],
			[]
		);
		expect(groups.map((g) => [g.label, g.lines.length])).toEqual([
			['personal', 2],
			['work', 1]
		]);
	});
});
