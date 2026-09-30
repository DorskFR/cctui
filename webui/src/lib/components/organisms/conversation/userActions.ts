import type { UserAction } from '@bindings/UserAction';

export interface UserActionGroups {
	open: UserAction[];
	resolved: UserAction[];
	/** Open items the agent cannot continue without. */
	blocking: number;
}

function stamp(iso: string | null | undefined): number {
	if (!iso) return 0;
	const t = new Date(iso).getTime();
	return Number.isNaN(t) ? 0 : t;
}

/**
 * Split a session's list into what the user still has to do and what is
 * already resolved. Re-sorts rather than trusting the payload order: the list
 * arrives from both an HTTP read and a ws push, and blocking items must lead
 * either way. `null` when there is nothing to render at all — including a list
 * a server too old to send one left undefined.
 */
export function groupUserActions(list: UserAction[] | undefined): UserActionGroups | null {
	if (!list?.length) return null;
	const open = list
		.filter((a) => a.status === 'open')
		.sort((a, b) => {
			if (a.blocking !== b.blocking) return a.blocking ? -1 : 1;
			return stamp(a.created_at) - stamp(b.created_at);
		});
	const resolved = list
		.filter((a) => a.status !== 'open')
		.sort((a, b) => stamp(a.resolved_at) - stamp(b.resolved_at));
	return { open, resolved, blocking: open.filter((a) => a.blocking).length };
}
