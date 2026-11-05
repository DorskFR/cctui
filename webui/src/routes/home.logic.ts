// Pure helpers for the overview page — no Svelte/reactive state, so they live
// outside the component and are unit-testable on their own.
import type { WindowTokenUsage } from '@bindings/WindowTokenUsage';
import type { TokenUsage } from '@bindings/TokenUsage';
import type { SessionStats } from '@bindings/SessionStats';
import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
import type { SessionListItem } from '@bindings/SessionListItem';

// Adapt a window's {input, output, cache_read} to the shape TokenUsage.svelte
// renders (the session-card readout). cost_usd / cache_creation are unused here.
export const asUsage = (w: WindowTokenUsage | undefined): TokenUsage => ({
	tokens_in: w?.input ?? 0,
	tokens_out: w?.output ?? 0,
	cost_usd: 0,
	cache_read_tokens: w?.cache_read ?? 0,
	cache_creation_tokens: 0
});

export type MetricKey = 'working' | 'needs_input' | 'machines' | 'sessions';

/** `/sessions` filtered to one list bucket. */
export const BUCKET_HREF = {
	working: '/sessions?status=working',
	needs_input: '/sessions?status=needs-input'
} as const;

export interface MetricTileData {
	key: MetricKey;
	value: number;
	/** Smaller, fainter tail right after the value — the `/5` of `3/5`. */
	suffix?: string;
	warn?: boolean;
	/** Second number folded into the label (archived sessions). */
	sub?: number;
	href?: string;
}

export function machinesOnline(rows: readonly MachineResourcesRow[]): { online: number; total: number } {
	return { online: rows.filter((m) => m.liveness === 'online').length, total: rows.length };
}

/** The four headline tiles of the Usage page, in display order. Session
 *  counts use the list's buckets so the tiles and /sessions agree. */
export function buildMetricTiles(
	stats: SessionStats | undefined,
	machines: readonly MachineResourcesRow[]
): MetricTileData[] {
	const needs = stats?.needs_input ?? 0;
	const { online, total } = machinesOnline(machines);
	return [
		{ key: 'working', value: stats?.working ?? stats?.live ?? 0, href: BUCKET_HREF.working },
		{ key: 'needs_input', value: needs, warn: needs > 0, href: BUCKET_HREF.needs_input },
		{ key: 'machines', value: online, suffix: `/${total}` },
		{ key: 'sessions', value: stats?.total ?? 0, sub: stats?.archived ?? 0, href: '/sessions' }
	];
}

/** Where the quiet "Open" next to Needs input goes: the one waiting session
 *  itself, or the filtered list when several wait. */
export function needsInputTarget(
	sessions: readonly Pick<SessionListItem, 'id' | 'attention'>[] | undefined
): string | null {
	const waiting = (sessions ?? []).filter((s) => s.attention === 'needs_input');
	if (waiting.length === 0) return null;
	return waiting.length === 1 ? `/sessions/${waiting[0].id}` : BUCKET_HREF.needs_input;
}
