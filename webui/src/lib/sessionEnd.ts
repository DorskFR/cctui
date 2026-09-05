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
	/** Badge text: the label, plus the first line of the detail for a failed start. */
	badge: string;
};

export const FAILED_START_REASONS: ReadonlySet<SessionEndReason> = new Set([
	'resume_failed',
	'spawn_failed'
]);

const BADGE_DETAIL_MAX = 48;

export function endBadgeText(reason: SessionEndReason, label: string, detail: string | null): string {
	if (!detail || !FAILED_START_REASONS.has(reason)) return label;
	const line = detail.split('\n', 1)[0].trim();
	const short = line.length > BADGE_DETAIL_MAX ? `${line.slice(0, BADGE_DETAIL_MAX - 1)}…` : line;
	return `${label}: ${short}`;
}

export function endReasonTone(reason: SessionEndReason): EndTone {
	switch (reason) {
		case 'completed':
			return 'ok';
		case 'crashed':
		case 'resume_failed':
		case 'spawn_failed':
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
		case 'spawn_failed':
			return m.sessions_end_spawn_failed();
		default:
			return m.sessions_end_other();
	}
}

/** Badge model for an ended session; `null` while it is alive or when it
 *  simply completed — a normal end is the row's resting state, not a badge. */
export function sessionEnd(s: Pick<SessionListItem, 'end_reason' | 'end_detail' | 'ended_at'>): SessionEnd | null {
	const reason = s.end_reason ?? null;
	if (!reason || reason === 'completed') return null;
	const label = endReasonLabel(reason);
	const detail = s.end_detail?.trim() || null;
	return {
		reason,
		label,
		tone: endReasonTone(reason),
		muted: reason === 'reaped_inactive',
		detail,
		endedAt: s.ended_at ?? null,
		badge: endBadgeText(reason, label, detail)
	};
}

/** Tooltip text: when it ended plus the adapter's diagnostic, if any. */
export function sessionEndTitle(end: SessionEnd): string {
	const at = end.endedAt ? new Date(end.endedAt).toLocaleString() : '—';
	const head = m.sessions_end_title({ at });
	return end.detail ? `${head}\n${end.detail}` : head;
}
