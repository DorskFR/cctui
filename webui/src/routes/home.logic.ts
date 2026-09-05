// Pure helpers for the overview page — no Svelte/reactive state, so they live
// outside the component and are unit-testable on their own.
import type { WindowTokenUsage } from '@bindings/WindowTokenUsage';
import type { TokenUsage } from '@bindings/TokenUsage';
import type { SessionStats } from '@bindings/SessionStats';
import type { MachineRow } from '@bindings/MachineRow';

// Adapt a window's {input, output, cache_read} to the shape TokenUsage.svelte
// renders (the session-card readout). cost_usd / cache_creation are unused here.
export const asUsage = (w: WindowTokenUsage | undefined): TokenUsage => ({
	tokens_in: w?.input ?? 0,
	tokens_out: w?.output ?? 0,
	cost_usd: 0,
	cache_read_tokens: w?.cache_read ?? 0,
	cache_creation_tokens: 0
});

export type MetricKey = 'live' | 'needs_input' | 'machines' | 'sessions';

export interface MetricTileData {
	key: MetricKey;
	value: number;
	/** Smaller, fainter tail right after the value — the `/5` of `3/5`. */
	suffix?: string;
	warn?: boolean;
	/** Second number folded into the label (archived sessions). */
	sub?: number;
}

export function machinesOnline(rows: readonly MachineRow[]): { online: number; total: number } {
	const live = rows.filter((m) => !m.revoked_at);
	return { online: live.filter((m) => m.liveness === 'online').length, total: live.length };
}

/** The four headline tiles of the Usage page, in display order. */
export function buildMetricTiles(
	stats: SessionStats | undefined,
	machines: readonly MachineRow[]
): MetricTileData[] {
	const needs = stats?.needs_input ?? 0;
	const { online, total } = machinesOnline(machines);
	return [
		{ key: 'live', value: stats?.live ?? 0 },
		{ key: 'needs_input', value: needs, warn: needs > 0 },
		{ key: 'machines', value: online, suffix: `/${total}` },
		{ key: 'sessions', value: stats?.total ?? 0, sub: stats?.archived ?? 0 }
	];
}
