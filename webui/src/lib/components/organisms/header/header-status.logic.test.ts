import { describe, expect, it } from 'vitest';
import type { AccountUsageEntry, UsageWindow } from '$lib/queries';
import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
import {
	DEFAULT_WARN_PCT,
	machineHealth,
	usageTone,
	windowShort,
	worseTone,
	worstUsage
} from './header-status.logic';

const win = (key: string, utilization: number, extra: Partial<UsageWindow> = {}): UsageWindow => ({
	key,
	kind: key.startsWith('weekly') ? 'weekly_all' : key,
	label: key,
	utilization,
	...extra
});

const entry = (id: string, windows: UsageWindow[], pin = true): AccountUsageEntry =>
	({
		account: `acct-${id}`,
		account_name: `Account ${id}`,
		account_emoji: null,
		header_pin: pin,
		account_id: id,
		provider: 'anthropic',
		usage: null,
		windows,
		age_secs: 0,
		limit_reset: null
	}) as unknown as AccountUsageEntry;

const noCaps = () => null;

describe('windowShort', () => {
	it('names the 5-hour and weekly windows by their length', () => {
		expect(windowShort(win('session', 1))).toBe('5h');
		expect(windowShort(win('weekly_all', 1))).toBe('7d');
		expect(windowShort({ key: 'other', kind: 'other', label: 'Monthly' })).toBe('Monthly');
	});
});

describe('usageTone', () => {
	it('stays neutral below the soft limit and turns amber past it', () => {
		expect(usageTone(70, 75)).toBe('ok');
		expect(usageTone(75, 75)).toBe('warn');
	});
	it('falls back to the default threshold without a soft limit', () => {
		expect(usageTone(DEFAULT_WARN_PCT - 1, null)).toBe('ok');
		expect(usageTone(DEFAULT_WARN_PCT, null)).toBe('warn');
	});
	it('is red only once the window is spent', () => {
		expect(usageTone(100, 50)).toBe('danger');
	});
});

describe('worstUsage', () => {
	it('returns null when no pinned provider reports a window', () => {
		expect(worstUsage(undefined, noCaps)).toBeNull();
		expect(worstUsage([entry('a', [win('session', 99)], false)], noCaps)).toBeNull();
	});

	it('picks the fullest window across pinned providers', () => {
		const rows = [entry('a', [win('session', 20), win('weekly_all', 88)]), entry('b', [win('session', 40)])];
		expect(worstUsage(rows, noCaps)).toMatchObject({ short: '7d', pct: 88, tone: 'ok', accountName: 'Account a' });
	});

	it('prefers a window past its soft limit over a fuller one that is not', () => {
		const rows = [entry('a', [win('weekly_all', 85)]), entry('b', [win('session', 60)])];
		const caps = (id: string) => (id === 'b' ? { session: { cap_pct: 50 } } : null);
		expect(worstUsage(rows, caps)).toMatchObject({ short: '5h', pct: 60, tone: 'warn' });
	});

	it('ignores dollar windows', () => {
		const rows = [entry('a', [win('spend', 0, { amount_usd: 12 }), win('session', 10)])];
		expect(worstUsage(rows, noCaps)).toMatchObject({ short: '5h', pct: 10 });
	});
});

describe('machineHealth', () => {
	const now = Date.parse('2026-11-05T12:00:00Z');
	const row = (cpu: number | null, updatedAt: string | null = '2026-11-05T11:59:00Z') =>
		({
			machine_id: String(cpu),
			name: 'm',
			display_name: null,
			updated_at: updatedAt,
			resources:
				cpu === null
					? null
					: { cpu_pct: cpu, mem_pct: 10, disk_pct: 10, load1: null, mem_used_bytes: 0, mem_total_bytes: 0, disk_used_bytes: 0, disk_total_bytes: 0, disk_path: '/' }
		}) as unknown as MachineResourcesRow;

	it('is null when no machine is monitored', () => {
		expect(machineHealth([], now)).toBeNull();
	});
	it('takes the worst machine', () => {
		expect(machineHealth([row(10), row(95), row(75)], now)).toBe('danger');
	});
	it('reads a quiet machine as unknown, but a hot one still wins', () => {
		expect(machineHealth([row(10), row(10, '2026-11-05T10:00:00Z')], now)).toBe('unknown');
		expect(machineHealth([row(null), row(10)], now)).toBe('unknown');
		expect(machineHealth([row(null), row(80)], now)).toBe('warn');
	});
});

describe('worseTone', () => {
	it('keeps whichever signal is present and the worse of two', () => {
		expect(worseTone(null, null)).toBeNull();
		expect(worseTone('ok', null)).toBe('ok');
		expect(worseTone(null, 'warn')).toBe('warn');
		expect(worseTone('danger', 'warn')).toBe('danger');
	});
});
