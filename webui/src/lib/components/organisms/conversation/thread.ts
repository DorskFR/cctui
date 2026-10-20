import type { SessionListItem } from '@bindings/SessionListItem';
import type { ThreadSession } from '@bindings/ThreadSession';
import { previousSessionId } from '$lib/followup';

export interface ThreadEntry {
	id: string;
	/** Unique within the thread: the menu keys its rows by label. */
	label: string;
	title: string;
	at: string | null;
	current: boolean;
}

type ThreadSource = Pick<
	SessionListItem,
	'id' | 'name' | 'working_dir' | 'parent_id' | 'metadata' | 'next_session_id' | 'registered_at'
>;

function uniqueLabels(entries: Omit<ThreadEntry, 'label'>[]): ThreadEntry[] {
	const seen = new Set<string>();
	return entries.map((e, i) => {
		let label = e.title;
		if (seen.has(label)) label = `${e.title} (${i + 1})`;
		seen.add(label);
		return { ...e, label };
	});
}

/** `fallback` names the session's own links until the fetched thread lands. */
export function threadEntries(
	session: ThreadSource,
	thread: readonly ThreadSession[] | undefined,
	fallback: { previous: string; next: string }
): ThreadEntry[] {
	if (thread && thread.length > 1 && thread.some((t) => t.id === session.id)) {
		return uniqueLabels(
			thread.map((t) => ({
				id: t.id,
				title: t.name || t.working_dir,
				at: t.registered_at,
				current: t.id === session.id
			}))
		);
	}
	const previous = previousSessionId(session);
	const next = session.next_session_id ?? null;
	if (!previous && !next) return [];
	return uniqueLabels([
		...(previous ? [{ id: previous, title: fallback.previous, at: null, current: false }] : []),
		{
			id: session.id,
			title: session.name || session.working_dir,
			at: session.registered_at ?? null,
			current: true
		},
		...(next ? [{ id: next, title: fallback.next, at: null, current: false }] : [])
	]);
}
