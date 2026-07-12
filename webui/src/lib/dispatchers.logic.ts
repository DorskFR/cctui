// Pure helpers for the dispatchers page — no Svelte/reactive state, so they live
// outside the component and are unit-testable on their own.
import type { UserDispatcher } from '$lib/queries';
import { m } from '$lib/paraglide/messages';

/** Human-readable liveness label, factoring in whether a WS is connected. */
export function livenessLabel(d: UserDispatcher): string {
	if (d.connected) return m.dispatch_liveness_connected();
	switch (d.liveness) {
		case 'online':
			return m.dispatch_liveness_online();
		case 'stale':
			return m.dispatch_liveness_stale();
		case 'offline':
			return m.dispatch_liveness_offline();
		default:
			return d.liveness; // online | stale | offline (last_seen-derived)
	}
}

/** Tone for the liveness badge — `connected`/`online` positive, `stale`
 *  cautionary, `offline` neutral. */
export function livenessTone(d: UserDispatcher): 'ok' | 'warn' | 'neutral' {
	if (d.connected || d.liveness === 'online') return 'ok';
	if (d.liveness === 'stale') return 'warn';
	return 'neutral';
}
