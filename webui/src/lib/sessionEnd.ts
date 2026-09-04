import type { SessionEndReason } from '@bindings/SessionEndReason';
import type { SessionListItem } from '@bindings/SessionListItem';
import { m } from '$lib/paraglide/messages';

export type EndTone = 'neutral' | 'ok' | 'warn' | 'danger' | 'info';

export type SessionEnd = {
	reason: SessionEndReason;
	label: string;
	tone: EndTone;
	/** Reaped sessions aged out silently — render them faded, not as a state. */
	muted: boolean;
	detail: string | null;
	endedAt: string | null;
};

export function endReasonTone(reason: SessionEndReason): EndTone {
	switch (reason) {
		case 'completed':
			return 'ok';
		case 'crashed':
		case 'resume_failed':
			return 'danger';
		case 'daemon_lost':
		case 'machine_offline':
			return 'warn';
		default:
			return 'neutral';
	}
}

export function endReasonLabel(reason: SessionEndReason): string {
	switch (reason) {
		case 'completed':
			return m.sessions_end_completed();
		case 'killed':
			return m.sessions_end_killed();
		case 'crashed':
			return m.sessions_end_crashed();
		case 'daemon_lost':
			return m.sessions_end_daemon_lost();
		case 'machine_offline':
			return m.sessions_end_machine_offline();
		case 'reaped_inactive':
			return m.sessions_end_reaped_inactive();
		case 'resume_failed':
			return m.sessions_end_resume_failed();
		default:
			return m.sessions_end_other();
	}
}

/** Badge model for an ended session; `null` while it is alive. */
export function sessionEnd(s: Pick<SessionListItem, 'end_reason' | 'end_detail' | 'ended_at'>): SessionEnd | null {
	const reason = s.end_reason ?? null;
	if (!reason) return null;
	return {
		reason,
		label: endReasonLabel(reason),
		tone: endReasonTone(reason),
		muted: reason === 'reaped_inactive',
		detail: s.end_detail?.trim() || null,
		endedAt: s.ended_at ?? null
	};
}

/** Tooltip text: when it ended plus the adapter's diagnostic, if any. */
export function sessionEndTitle(end: SessionEnd): string {
	const at = end.endedAt ? new Date(end.endedAt).toLocaleString() : '—';
	const head = m.sessions_end_title({ at });
	return end.detail ? `${head}\n${end.detail}` : head;
}
