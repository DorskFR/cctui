import type { SessionListItem } from '@bindings/SessionListItem';

const same = (a: string, b: string) => a.toLocaleLowerCase() === b.toLocaleLowerCase();

/**
 * A session's display name: a user-set name with the agent's own title beside
 * it, `NAME (agent title)`. An agent-owned name is the agent title already.
 */
export function sessionDisplayName(
	name: string | null | undefined,
	agentTitle: string | null | undefined,
	source: string | null | undefined
): string {
	const n = name?.trim() ?? '';
	const t = agentTitle?.trim() ?? '';
	if (!n) return source === 'user' ? t : '';
	if (source !== 'user' || !t || same(n, t)) return n;
	return `${n} (${t})`;
}

function metaString(meta: unknown, key: string): string | null {
	if (!meta || typeof meta !== 'object' || Array.isArray(meta)) return null;
	const v = (meta as Record<string, unknown>)[key];
	return typeof v === 'string' ? v : null;
}

/** The stored name, else the title the user pinned when the row lost it. */
export function nameOf(s: Pick<SessionListItem, 'name' | 'metadata'>): string {
	const n = s.name?.trim();
	if (n) return n;
	if (metaString(s.metadata, 'name_source') !== 'user') return '';
	return metaString(s.metadata, 'custom_title')?.trim() ?? '';
}

export function displayNameOf(s: Pick<SessionListItem, 'name' | 'metadata'>): string {
	return sessionDisplayName(
		nameOf(s),
		metaString(s.metadata, 'agent_title'),
		metaString(s.metadata, 'name_source')
	);
}

/** Conversation header title: the session name, the working folder otherwise. */
export function headerTitleOf(
	s: Pick<SessionListItem, 'name' | 'metadata' | 'working_dir'>
): string {
	return displayNameOf(s) || s.working_dir;
}
