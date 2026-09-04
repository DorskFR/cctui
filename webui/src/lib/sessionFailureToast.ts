import type { SessionEndedEvent } from '$lib/ws.svelte';
import { FAILED_START_REASONS, endReasonLabel } from '$lib/sessionEnd';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';

const TOAST_DETAIL_MAX = 240;

/** Where a failure toast's Diagnose action lands: the session with its panel open. */
export function diagnoseHref(sessionId: string): string {
	return `/sessions/${encodeURIComponent(sessionId)}?diagnose=1`;
}

/** Error toast for a session that failed to start or crashed; other ends are silent. */
export function sessionFailureToast(ev: SessionEndedEvent, navigate: (href: string) => void): boolean {
	if (ev.reason !== 'crashed' && !FAILED_START_REASONS.has(ev.reason)) return false;
	const raw = ev.detail?.trim() || m.spawn_error_unknown();
	const detail = raw.length > TOAST_DETAIL_MAX ? `${raw.slice(0, TOAST_DETAIL_MAX - 1)}…` : raw;
	toasts.err(m.sessions_end_failed_toast({ label: endReasonLabel(ev.reason), detail }), {
		label: m.sessions_end_diagnose(),
		run: () => navigate(diagnoseHref(ev.session_id))
	});
	return true;
}
