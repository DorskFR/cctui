import type { SessionEndedEvent } from '$lib/ws.svelte';
import { endReasonLabel, isFailedStart } from '$lib/sessionEnd';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';

const TOAST_DETAIL_MAX = 240;

export function sessionHref(sessionId: string): string {
	return `/sessions/${encodeURIComponent(sessionId)}`;
}

/** Whether an end reason warrants a toast at all; other ends are silent. */
export function shouldToast(reason: SessionEndedEvent['reason']): boolean {
	return reason === 'crashed' || isFailedStart(reason);
}

/** The toast's detail line, or null when the event carried nothing and the
 *  caller must substitute its own "unknown error" wording. */
export function toastDetail(detail: string | null | undefined): string | null {
	const raw = detail?.trim();
	if (!raw) return null;
	return raw.length > TOAST_DETAIL_MAX ? `${raw.slice(0, TOAST_DETAIL_MAX - 1)}…` : raw;
}

/** Error toast for a session that failed to start or crashed; other ends are silent. */
export function sessionFailureToast(ev: SessionEndedEvent, navigate: (href: string) => void): boolean {
	if (!shouldToast(ev.reason)) return false;
	const detail = toastDetail(ev.detail) ?? m.spawn_error_unknown();
	toasts.error(m.sessions_end_failed_toast({ label: endReasonLabel(ev.reason), detail }), undefined, {
		label: m.sessions_end_open(),
		run: () => navigate(sessionHref(ev.session_id))
	});
	return true;
}
