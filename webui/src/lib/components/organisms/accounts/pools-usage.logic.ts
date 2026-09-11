import type { PoolUsageMember } from '@bindings/PoolUsageMember';
import type { PoolUsageWindow } from '@bindings/PoolUsageWindow';
import type { UsagePace } from '$lib/queries';
import { windowLabelFromKey } from '$lib/components/molecules/usage-windows';

// Pure helpers behind the pool gauges: the server aggregates, this file only
// shapes its answer for the same bars and glyphs the account cards use.

/** Display name of a provider family: the harness people know it by. */
export function familyLabel(family: string): string {
	switch (family) {
		case 'anthropic':
			return 'Claude';
		case 'openai':
			return 'Codex';
		case 'fireworks':
			return 'Fireworks';
		default:
			return family;
	}
}

/** Row label of a pool window: the model name for a scoped weekly window,
 *  the short canonical label otherwise. */
export function poolWindowLabel(window: PoolUsageWindow): string {
	if (window.kind === 'weekly_scoped' && window.model_display_name) return window.model_display_name;
	return windowLabelFromKey(window.key);
}

/** The pool window as a pace, so `paceState` / `wallInMs` read it exactly
 *  like a single account's window. Null when the pool has no linear budget to
 *  compare against yet (every member at the very start of its window). */
export function poolWindowPace(window: PoolUsageWindow): UsagePace | null {
	if (window.ratio === null || !Number.isFinite(window.ratio)) return null;
	return {
		elapsed_fraction: Math.max(0, Math.min(1, window.expected_pct / 100)),
		expected_pct: window.expected_pct,
		ratio: window.ratio,
		projected_wall_at: window.projection?.wall_at ?? undefined,
		slope_hours: window.projection?.slope_hours ?? null
	};
}

/** What the projection says, before wording: the pool hits its wall in `ms`,
 *  it holds until the members reset, or there is not enough history to tell. */
export type ProjectionState =
	| { kind: 'wall'; ms: number }
	| { kind: 'holds' }
	| { kind: 'insufficient' };

export function projectionState(window: PoolUsageWindow, now: number): ProjectionState {
	const p = window.projection;
	if (!p) return { kind: 'insufficient' };
	if (!p.wall_at) return { kind: 'holds' };
	const wall = Date.parse(p.wall_at);
	if (!Number.isFinite(wall)) return { kind: 'holds' };
	return { kind: 'wall', ms: Math.max(0, wall - now) };
}

/** `4 · 4 · 1` in member order, or null when every member weighs 1 — the
 *  default needs no note. */
export function weightsNote(members: PoolUsageMember[]): string | null {
	if (members.length === 0 || members.every((mem) => mem.weight === 1)) return null;
	return members.map((mem) => trimWeight(mem.weight)).join(' · ');
}

function trimWeight(w: number): string {
	return Number.isInteger(w) ? String(w) : w.toFixed(1).replace(/\.0$/, '');
}

/** `3.2` for a demand in points per hour, `0.5` under one. */
export function fmtDemand(pctPerHour: number): string {
	return pctPerHour.toFixed(pctPerHour >= 10 ? 0 : 1);
}
