import type { SessionListItem } from '@bindings/SessionListItem';

type Recent = Pick<SessionListItem, 'id' | 'status' | 'parent_id' | 'last_activity_at' | 'registered_at'>;

const activity = (s: Recent) => Date.parse(s.last_activity_at ?? s.registered_at ?? '') || 0;

/** The top-level, unarchived sessions touched most recently. */
export function recentSessions<T extends Recent>(list: readonly T[] | undefined, limit = 5): T[] {
	return (list ?? [])
		.filter((s) => s.status !== 'archived' && !s.parent_id)
		.toSorted((a, b) => activity(b) - activity(a))
		.slice(0, limit);
}

export const isNotFound = (status: number) => status === 404;
