import { useAccounts, useAllAccountsUsage } from '#lib/queries/index.ts';
import { m } from '#lib/paraglide/messages.js';
import { batteryEntries, type BatteryEntry } from './usage-battery.logic.ts';
import {
	entryReadings,
	worstReading,
	type CapOf,
	type GaugeReading,
	type GaugeWindow
} from './usage-gauge.logic.ts';

export interface GaugeRow {
	entry: BatteryEntry;
	readings: GaugeReading[];
}

export const windowShort = (w: GaugeWindow) =>
	w === 'session' ? m.usage_battery_five_hour() : m.usage_gauge_weekly_short();

export const readingText = (r: GaugeReading) => `${windowShort(r.window)} ${r.pct}%`;

/** Call during component init. */
export function useUsageGauge() {
	const usage = useAllAccountsUsage(() => true);
	const accounts = useAccounts();

	const capOf: CapOf = (providerId, window) => {
		for (const a of accounts.data ?? []) {
			const p = a.providers.find((p) => p.id === providerId);
			if (p) return p.soft_limits?.[window]?.cap_pct ?? null;
		}
		return null;
	};

	const rows = $derived<GaugeRow[]>(
		batteryEntries(usage.data).map((entry) => ({ entry, readings: entryReadings(entry, capOf) }))
	);
	const worst = $derived(worstReading(rows.flatMap((r) => r.readings)));

	return {
		get rows() {
			return rows;
		},
		get worst() {
			return worst;
		}
	};
}
