import type { SessionListItem } from '@bindings/SessionListItem';
import { m } from '#lib/paraglide/messages.js';

export function errorCopy(
	status: number,
	missingSession: boolean,
	path: string
): { title: string; body: string } {
	if (missingSession)
		return { title: m.error_session_missing_title(), body: m.error_session_missing_body() };
	if (status === 404) return { title: m.error_not_found_title(), body: m.error_not_found_body({ path }) };
	return { title: m.error_failed_title(), body: m.error_failed_body() };
}

const lastSeen = (s: SessionListItem) => Date.parse(s.last_activity_at ?? s.last_message_at ?? '') || 0;

export function recentSessions(list: SessionListItem[], n: number): SessionListItem[] {
	return list
		.filter((s) => s.status !== 'archived')
		.slice().sort((a, b) => lastSeen(b) - lastSeen(a))
		.slice(0, n);
}

export function matchSessions(
	list: SessionListItem[],
	query: string,
	nameOf: (s: SessionListItem) => string
): SessionListItem[] {
	const q = query.trim().toLowerCase();
	if (!q) return [];
	return list.filter((s) =>
		[nameOf(s), s.working_dir, s.id].some((v) => v.toLowerCase().includes(q))
	);
}
