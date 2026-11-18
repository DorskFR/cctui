import type { InboxEvent } from '#lib/queries/inbox.ts';

export type RunState = 'queued' | 'running' | 'delivered' | 'skipped' | 'cancelled' | 'failed';

export interface RunRow {
	id: string;
	state: RunState;
	at: number;
	manual: boolean;
	sessionId: string | null;
	error: string | null;
}

const SKIPPED = 'skipped:';

export const presetLane = (presetId: string): string => `preset:${presetId}:default`;

export function runState(event: Pick<InboxEvent, 'status' | 'last_error'>): RunState {
	switch (event.status) {
		case 'pending':
			return 'queued';
		case 'claimed':
			return 'running';
		case 'delivered':
			return 'delivered';
		case 'dead':
			return 'failed';
		default:
			return event.last_error?.startsWith(SKIPPED) ? 'skipped' : 'cancelled';
	}
}

export function runRow(event: InboxEvent): RunRow {
	const state = runState(event);
	return {
		id: event.id,
		state,
		at: Date.parse(event.created_at),
		manual: event.payload?.manual === true,
		sessionId: event.delivered_session_id,
		error: state === 'skipped' || state === 'delivered' ? null : event.last_error
	};
}

export function runRows(events: readonly InboxEvent[], limit = 10): RunRow[] {
	return events
		.map(runRow)
		.sort((a, b) => b.at - a.at)
		.slice(0, limit);
}

export const RUN_TONE: Record<RunState, 'warn' | 'info' | 'ok' | 'muted' | 'danger'> = {
	queued: 'warn',
	running: 'info',
	delivered: 'ok',
	skipped: 'muted',
	cancelled: 'muted',
	failed: 'danger'
};
